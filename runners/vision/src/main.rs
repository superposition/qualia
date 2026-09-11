//! `qualia-vision`: the visual cortex runner.
//!
//! The process attaches to the arena named by `QUALIA_SHM_NAME` and is the only
//! writer of the [`WorldModel`] and the room-scale [`WorldVoxels`] grid. Each
//! pass over the loop either builds a synthetic scene from the sensed belief
//! (offline) or sends one camera frame to Gemini for scene understanding plus a
//! semantic embedding (online). Detected objects are projected into the voxel
//! lattice, pending layer questions are answered into the lore ring, and the
//! scene embedding is republished into the senses layer.
//!
//! The wire format is the Gemini REST API: the vision call posts the frame as
//! `inline_data` beside a prompt and reads a schema-constrained JSON object back
//! from `candidates[0].content.parts[0].text`; the embedding call posts text and
//! reads `embedding.values`. Both calls can be pointed at a fake
//! [`Transport`], so the behaviour of this crate is tested without a network.

use base64::Engine as _;
use qualia_shm::{LayerReader, LayerWriter, ShmRegion};
use qualia_types::*;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant, SystemTime};

// ---------------------------------------------------------------------------
// Configuration and constants
// ---------------------------------------------------------------------------

/// Arena used when `QUALIA_SHM_NAME` is unset.
const DEFAULT_SHM_NAME: &str = "/qualia_body";
/// Seconds between model calls when `QUALIA_LLM_INTERVAL` is unset or malformed.
const DEFAULT_LLM_INTERVAL_SECS: u64 = 30;
/// Model calls per session when `QUALIA_LLM_MAX_CALLS` is unset or malformed.
const DEFAULT_LLM_MAX_CALLS: u64 = 50;

/// Frame size requested from the capture device.
const CAPTURE_WIDTH: u32 = 640;
const CAPTURE_HEIGHT: u32 = 480;

/// Gemini model used for scene understanding.
const VISION_MODEL: &str = "gemini-2.5-flash";
/// Gemini model used for the semantic embedding.
const EMBEDDING_MODEL: &str = "gemini-embedding-2-preview";
/// Root of the Gemini REST endpoints.
const API_ROOT: &str = "https://generativelanguage.googleapis.com/v1beta/models";
const VISION_MAX_TOKENS: u32 = 2048;
const VISION_TEMPERATURE: f32 = 0.1;

/// Pause between loop passes.
const LOOP_PERIOD_MS: u64 = 200;

/// Directive written at startup when the operator has not chosen one.
const DEFAULT_DIRECTIVE: &str = "Observe and understand the environment. Report what you see.";

/// Thought kinds used by this runner.
const THOUGHT_OBSERVE: u8 = 0;
const THOUGHT_LEARN: u8 = 3;
const THOUGHT_ESCALATE: u8 = 5;
/// Layer id the runner speaks under; no belief layer owns it.
const RUNNER_LAYER: u8 = 255;

/// Occupancy shed by every cell on each voxel update.
const VOXEL_DECAY: u8 = 20;
/// Smallest room dimension believed instead of falling back to a default.
const MIN_ROOM_M: f32 = 0.5;
const DEFAULT_ROOM_W: f32 = 6.0;
const DEFAULT_ROOM_D: f32 = 6.0;
const DEFAULT_ROOM_H: f32 = 2.5;
/// Smallest object depth believed instead of estimating it from the frame.
const MIN_DEPTH_M: f32 = 0.1;
const DEPTH_FROM_FRAME_BASE: f32 = 0.3;
const DEPTH_FROM_FRAME_SPAN: f32 = 0.85;
/// tan of half the assumed 90 degree horizontal field of view.
const HALF_FOV_TAN: f32 = 1.0;
/// Height above the floor at which a detected object is assumed to sit.
const OBJECT_HEIGHT_M: f32 = 0.5;
const VOXEL_STAMP_RADIUS: i32 = 1;
const VOXEL_STAMP_HEIGHT: i32 = 2;
const MIN_COLOR: u8 = 80;
const OCCUPANCY_BASE: u8 = 55;
const OCCUPANCY_SCALE: f32 = 200.0;

/// Precision published for a weak embedding dimension.
const PRECISION_FLOOR: f32 = 0.1;
/// Extra precision granted per unit of embedding strength.
const PRECISION_GAIN: f32 = 0.9;

/// Sensor variance above which the offline scene calls itself moving.
const MOTION_VARIANCE: f32 = 0.02;
/// Sensor variance above which the offline embedding raises its motion flag.
const EMBED_FLAG_VARIANCE: f32 = 0.01;
/// Brightness above which the offline scene calls itself lit.
const LIT_BRIGHTNESS: f32 = 0.3;
/// Brightness change above which an offline pass reports the sensor.
const SENSOR_DELTA: f32 = 0.1;
/// Offline passes between sensor thoughts.
const OFFLINE_THOUGHT_INTERVAL: u64 = 30;
/// Offline passes between the quiet sensor notes that stand in for a still scene.
const OFFLINE_FALLBACK_INTERVAL: u64 = 150;
/// Offline passes between tick summaries on stderr.
const OFFLINE_STDERR_INTERVAL: u64 = 60;
/// Sweep rate of the synthetic object drift, in radians per pass.
const OFFLINE_SWEEP_RATE: f32 = 0.035;

/// Longest excerpt of a scene description carried in a thought.
const SCENE_EXCERPT: usize = 80;
/// Longest excerpt of a question carried in a thought.
const QUESTION_EXCERPT: usize = 50;
/// Longest excerpt of an answer carried in a thought.
const ANSWER_EXCERPT: usize = 60;

/// Longest excerpt of a malformed response carried in an error message.
const ERROR_EXCERPT: usize = 200;

/// The runner's environment-derived settings.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Config {
    shm_name: String,
    api_key: String,
    llm_interval_secs: u64,
    llm_max_calls: u64,
}

impl Config {
    /// Reads the process environment.
    fn from_env() -> Self {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// Reads the documented keys through `lookup`, defaulting what is absent.
    fn from_lookup(mut lookup: impl FnMut(&str) -> Option<String>) -> Self {
        let shm_name = lookup("QUALIA_SHM_NAME").unwrap_or_else(|| DEFAULT_SHM_NAME.to_string());
        let api_key = lookup("GEMINI_API_KEY").unwrap_or_default();
        let llm_interval_secs = number(&mut lookup, "QUALIA_LLM_INTERVAL")
            .unwrap_or(DEFAULT_LLM_INTERVAL_SECS);
        let llm_max_calls =
            number(&mut lookup, "QUALIA_LLM_MAX_CALLS").unwrap_or(DEFAULT_LLM_MAX_CALLS);
        Self {
            shm_name,
            api_key,
            llm_interval_secs,
            llm_max_calls,
        }
    }

    /// Whether a Gemini key was supplied.
    fn online(&self) -> bool {
        !self.api_key.is_empty()
    }
}

/// A positive integer read from the environment, or `None`.
fn number(lookup: &mut impl FnMut(&str) -> Option<String>, key: &str) -> Option<u64> {
    lookup(key)?.trim().parse().ok()
}

// ---------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------

/// A JSON-over-HTTP POST, injectable so tests never touch the network.
trait Transport {
    /// Posts `json` to `url` and returns the response body.
    fn post_json(&self, url: &str, json: &str) -> Result<String, String>;
}

/// The real transport, backed by `ureq`.
struct HttpTransport;

impl Transport for HttpTransport {
    fn post_json(&self, url: &str, json: &str) -> Result<String, String> {
        let response = ureq::post(url)
            .set("content-type", "application/json")
            .send_string(json)
            .map_err(|error| format!("request failed: {error}"))?;
        response
            .into_string()
            .map_err(|error| format!("response body unreadable: {error}"))
    }
}

// ---------------------------------------------------------------------------
// Gemini wire types
// ---------------------------------------------------------------------------

/// Token accounting reported by one model call.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ModelUsage {
    input_tokens: u64,
    output_tokens: u64,
}

/// One object the vision model reports having seen.
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
struct VisionObject {
    /// Short label such as `mug`.
    name: String,
    /// Model confidence, `0.0..=1.0`.
    confidence: f32,
    /// Normalised horizontal position in the frame, `0.0..=1.0`.
    #[serde(default)]
    x: f32,
    /// Normalised vertical position in the frame, `0.0..=1.0`.
    #[serde(default)]
    y: f32,
    /// Distance from the camera in metres; zero when the model did not say.
    #[serde(default)]
    depth_m: f32,
}

/// Room dimensions the vision model estimated.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Deserialize)]
struct Room {
    #[serde(default)]
    width_m: f32,
    #[serde(default)]
    depth_m: f32,
    #[serde(default)]
    height_m: f32,
}

/// The schema-constrained JSON the vision model is asked to return.
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
struct VisionResponse {
    scene: String,
    activity: String,
    objects: Vec<VisionObject>,
    /// Answers to the harvested layer questions, in the order asked.
    #[serde(default)]
    lore_answers: Vec<String>,
    #[serde(default)]
    room: Room,
}

/// Present the parsed vision response and its token accounting.
type VisionCall = (VisionResponse, ModelUsage);

/// One unanswered question pulled out of a layer slot.
#[derive(Clone, Debug, PartialEq)]
struct PendingQuestion {
    layer: u8,
    reason: u8,
    text: String,
}

/// The URL of the vision endpoint.
fn vision_url(api_key: &str) -> String {
    format!("{API_ROOT}/{VISION_MODEL}:generateContent?key={api_key}")
}

/// The URL of the embedding endpoint.
fn embedding_url(api_key: &str) -> String {
    format!("{API_ROOT}/{EMBEDDING_MODEL}:embedContent?key={api_key}")
}

/// The prompt that asks for the scene schema, with the layer questions appended
/// when there are any.
fn vision_prompt(directive: &str, questions: &[PendingQuestion]) -> String {
    let mut prompt = String::from(
        "You are the visual cortex of an autonomous machine. Read the attached camera frame \
         and reply with one JSON object only: no prose, no code fences.\n\
         Required keys and shapes:\n\
         {\"scene\": \"<one sentence>\", \"activity\": \"<what is happening>\", \
         \"room\": {\"width_m\": 0.0, \"depth_m\": 0.0, \"height_m\": 0.0}, \
         \"objects\": [{\"name\": \"<label>\", \"confidence\": 0.0, \"x\": 0.0, \"y\": 0.0, \
         \"depth_m\": 0.0}]",
    );
    if !questions.is_empty() {
        prompt.push_str(", \"lore_answers\": [");
        for (index, question) in questions.iter().enumerate() {
            if index > 0 {
                prompt.push_str(", ");
            }
            prompt.push_str(&format!(
                "\"<answer to layer {}: {}>\"",
                question.layer, question.text
            ));
        }
        prompt.push(']');
        prompt.push_str(&format!(
            "}}\nThe perception layers asked {} question(s); answer them in order in lore_answers, \
             grounded in the frame.",
            questions.len()
        ));
    } else {
        prompt.push('}');
    }
    prompt.push_str("\nOperator directive: ");
    prompt.push_str(directive);
    prompt.push_str("\nStay factual and concise.");
    prompt
}

/// The generateContent body: the JPEG beside the text prompt.
fn vision_body(image_b64: &str, prompt: &str) -> String {
    serde_json::json!({
        "contents": [{
            "parts": [
                {"inline_data": {"mime_type": "image/jpeg", "data": image_b64}},
                {"text": prompt}
            ]
        }],
        "generationConfig": {
            "maxOutputTokens": VISION_MAX_TOKENS,
            "temperature": VISION_TEMPERATURE
        }
    })
    .to_string()
}

/// The embedContent body: the scene text, capped at the belief width.
fn embedding_body(text: &str) -> String {
    serde_json::json!({
        "model": format!("models/{EMBEDDING_MODEL}"),
        "content": {"parts": [{"text": text}]},
        "outputDimensionality": STATE_DIM
    })
    .to_string()
}

/// Strips an optional markdown fence from model output.
fn strip_fences(text: &str) -> &str {
    let trimmed = text.trim();
    let trimmed = trimmed.strip_prefix("```json").unwrap_or(trimmed);
    let trimmed = trimmed.strip_prefix("```").unwrap_or(trimmed);
    let trimmed = trimmed.strip_suffix("```").unwrap_or(trimmed);
    trimmed.trim()
}

/// A short, printable excerpt of a response, for error messages.
fn error_excerpt(raw: &str) -> String {
    raw.chars().take(ERROR_EXCERPT).collect()
}

/// The first `limit` bytes of `text`, never splitting a character.
fn excerpt(text: &str, limit: usize) -> &str {
    if text.len() <= limit {
        return text;
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Calls the vision model and parses its schema-constrained reply.
fn call_vision(
    transport: &impl Transport,
    api_key: &str,
    image_b64: &str,
    directive: &str,
    questions: &[PendingQuestion],
) -> Result<VisionCall, String> {
    let body = vision_body(image_b64, &vision_prompt(directive, questions));
    let raw = transport.post_json(&vision_url(api_key), &body)?;
    let envelope: serde_json::Value =
        serde_json::from_str(&raw).map_err(|error| format!("vision envelope is not JSON: {error}"))?;
    let text = envelope
        .pointer("/candidates/0/content/parts/0/text")
        .and_then(|value| value.as_str())
        .ok_or_else(|| {
            format!(
                "vision envelope has no text candidate: {}",
                error_excerpt(&raw)
            )
        })?;
    let response: VisionResponse = serde_json::from_str(strip_fences(text))
        .map_err(|error| format!("vision payload does not match the scene schema: {error}"))?;
    let usage = ModelUsage {
        input_tokens: envelope
            .pointer("/usageMetadata/promptTokenCount")
            .and_then(|value| value.as_u64())
            .unwrap_or(0),
        output_tokens: envelope
            .pointer("/usageMetadata/candidatesTokenCount")
            .and_then(|value| value.as_u64())
            .unwrap_or(0),
    };
    Ok((response, usage))
}

/// Calls the embedding model and returns the vector with its token count.
fn call_embedding(
    transport: &impl Transport,
    api_key: &str,
    text: &str,
) -> Result<(Vec<f32>, u64), String> {
    let raw = transport.post_json(&embedding_url(api_key), &embedding_body(text))?;
    let envelope: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|error| format!("embedding envelope is not JSON: {error}"))?;
    let values = envelope
        .pointer("/embedding/values")
        .and_then(|value| value.as_array())
        .ok_or_else(|| {
            format!(
                "embedding envelope has no values: {}",
                error_excerpt(&raw)
            )
        })?;
    let embedding: Vec<f32> = values
        .iter()
        .filter_map(|value| value.as_f64().map(|number| number as f32))
        .collect();
    if embedding.is_empty() {
        return Err("embedding envelope carried no numbers".to_string());
    }
    let tokens = envelope
        .pointer("/usageMetadata/totalTokenCount")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    Ok((embedding, tokens))
}

// ---------------------------------------------------------------------------
// Shared-memory effects
// ---------------------------------------------------------------------------

/// Copies `text` into a fixed buffer, zeroing it first and leaving a terminator.
fn write_cstr<const N: usize>(buffer: &mut [u8; N], text: &str) {
    buffer.fill(0);
    let bytes = text.as_bytes();
    let length = bytes.len().min(N.saturating_sub(1));
    buffer[..length].copy_from_slice(&bytes[..length]);
}

/// Reads a NUL-terminated buffer as a lossy string.
fn read_cstr(buffer: &[u8]) -> String {
    let end = buffer.iter().position(|&byte| byte == 0).unwrap_or(buffer.len());
    String::from_utf8_lossy(&buffer[..end]).into_owned()
}

/// Applies the parsed scene to the shared world model.
fn apply_vision_response(world: &mut WorldModel, response: &VisionResponse) {
    write_cstr(&mut world.scene, &response.scene);
    write_cstr(&mut world.activity, &response.activity);

    world.num_objects = 0;
    for object in response.objects.iter().take(MAX_OBJECTS) {
        let index = world.num_objects as usize;
        if index >= MAX_OBJECTS {
            break;
        }
        let slot = &mut world.objects[index];
        slot.name = [0u8; MAX_OBJECT_NAME];
        write_cstr(&mut slot.name, &object.name);
        slot.confidence = object.confidence;
        slot.x = object.x;
        slot.y = object.y;
        slot.active = 1;
        world.num_objects += 1;
    }
}

/// Fits an embedding of any length into the `STATE_DIM` scene vector.
fn project_embedding_to_scene(world: &mut WorldModel, embedding: &[f32]) {
    if embedding.len() == STATE_DIM {
        world.scene_embedding.copy_from_slice(embedding);
    } else if embedding.len() > STATE_DIM {
        for dim in 0..STATE_DIM {
            let start = dim * embedding.len() / STATE_DIM;
            let end = ((dim + 1) * embedding.len() / STATE_DIM)
                .max(start + 1)
                .min(embedding.len());
            let sum: f32 = embedding[start..end].iter().sum();
            world.scene_embedding[dim] = sum / (end - start) as f32;
        }
    } else {
        for dim in 0..STATE_DIM {
            world.scene_embedding[dim] = embedding.get(dim).copied().unwrap_or(0.0);
        }
    }
}

/// Deterministic local embedding used when the embedding endpoint is down.
fn hash_embedding(world: &mut WorldModel, response: &VisionResponse) {
    let text = format!("{} {}", response.scene, response.activity);
    for (dim, value) in world.scene_embedding.iter_mut().enumerate() {
        let mut accumulator = 0.0f32;
        for (index, byte) in text.bytes().enumerate() {
            accumulator +=
                byte as f32 * ((dim as f32 * 0.37 + index as f32 * 0.11).cos()) * 0.01;
        }
        for object in response.objects.iter().take(MAX_OBJECTS) {
            accumulator +=
                object.confidence * ((object.x + dim as f32) * 0.5).sin() * 0.1;
        }
        *value = accumulator.tanh();
    }
}

/// Euclidean distance between two scene embeddings.
fn embedding_delta(before: &[f32; STATE_DIM], after: &[f32; STATE_DIM]) -> f32 {
    (0..STATE_DIM)
        .map(|dim| (after[dim] - before[dim]).powi(2))
        .sum::<f32>()
        .sqrt()
}

/// Pulls every unanswered layer question out of the arena, clearing `pending`.
fn harvest_questions(shm: &ShmRegion) -> Vec<PendingQuestion> {
    let mut pending = Vec::new();
    for layer in 0..NUM_LAYERS {
        let slot = shm.layer_slot(layer);
        if !slot.question.pending.load(Ordering::Acquire) {
            continue;
        }
        slot.question.pending.store(false, Ordering::Release);
        let text = read_cstr(&slot.question.text);
        if text.is_empty() {
            continue;
        }
        pending.push(PendingQuestion {
            layer: slot.question.layer,
            reason: slot.question.reason,
            text,
        });
    }
    pending
}

/// Writes one lore entry and its thought per answered question, pairing the
/// question and answer lists by index.
fn record_lore(
    shm: &ShmRegion,
    questions: &[PendingQuestion],
    answers: &[String],
    embedding_delta: f32,
) {
    for (question, answer) in questions.iter().zip(answers.iter()) {
        shm.emit_lore(
            &question.text,
            answer,
            question.layer,
            question.reason,
            embedding_delta,
            0.0,
        );
        shm.emit_thought(
            RUNNER_LAYER,
            THOUGHT_LEARN,
            0.0,
            &format!(
                "lore L{} r={}: Q={} A={}",
                question.layer,
                question.reason,
                excerpt(&question.text, QUESTION_EXCERPT),
                excerpt(answer, ANSWER_EXCERPT)
            ),
        );
    }
}

/// Maps a metre offset from the room centre onto a voxel column.
fn centered_voxel(value_m: f32, span_m: f32, cells: usize) -> i32 {
    (((value_m / span_m) + 0.5) * cells as f32)
        .floor()
        .clamp(0.0, cells.saturating_sub(1) as f32) as i32
}

/// Maps a metre offset from the room edge onto a voxel column.
fn forward_voxel(value_m: f32, span_m: f32, cells: usize) -> i32 {
    ((value_m / span_m) * cells as f32)
        .floor()
        .clamp(0.0, cells.saturating_sub(1) as f32) as i32
}

/// A stable colour for an object label.
fn name_color(name: &str) -> (u8, u8, u8) {
    let mut hash = 0u32;
    for byte in name.bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(u32::from(byte));
    }
    (
        ((hash & 0xFF) as u8).max(MIN_COLOR),
        (((hash >> 8) & 0xFF) as u8).max(MIN_COLOR),
        (((hash >> 16) & 0xFF) as u8).max(MIN_COLOR),
    )
}

/// Paints a small box of occupancy around a voxel, keeping the stronger value.
fn stamp_voxel(
    voxels: &mut WorldVoxels,
    vx: i32,
    vy: i32,
    vz: i32,
    occupancy: u8,
    color: (u8, u8, u8),
) {
    let (r, g, b) = color;
    for dx in -VOXEL_STAMP_RADIUS..=VOXEL_STAMP_RADIUS {
        for dz in -VOXEL_STAMP_RADIUS..=VOXEL_STAMP_RADIUS {
            for dy in 0..=VOXEL_STAMP_HEIGHT {
                let ix = (vx + dx).clamp(0, VOXEL_W as i32 - 1) as usize;
                let iz = (vz + dz).clamp(0, VOXEL_D as i32 - 1) as usize;
                let iy = (vy + dy).clamp(0, VOXEL_H as i32 - 1) as usize;
                let cell = &mut voxels.cells[ix * VOXEL_D * VOXEL_H + iz * VOXEL_H + iy];
                if cell.occupancy < occupancy {
                    cell.occupancy = occupancy;
                    cell.r = r;
                    cell.g = g;
                    cell.b = b;
                }
            }
        }
    }
}

/// Where along the view axis an object sits, in metres.
fn object_depth(object: &VisionObject, room_depth_m: f32) -> f32 {
    if object.depth_m > MIN_DEPTH_M {
        object.depth_m.min(room_depth_m * 0.95)
    } else {
        DEPTH_FROM_FRAME_BASE
            + (1.0 - object.y.clamp(0.0, 1.0)) * room_depth_m * DEPTH_FROM_FRAME_SPAN
    }
}

/// Decays the lattice and projects every detected object into it.
fn update_world_voxels(voxels: &mut WorldVoxels, objects: &[VisionObject], room: &Room) {
    for cell in voxels.cells.iter_mut() {
        cell.occupancy = cell.occupancy.saturating_sub(VOXEL_DECAY);
    }

    let room_w = if room.width_m > MIN_ROOM_M {
        room.width_m
    } else {
        DEFAULT_ROOM_W
    };
    let room_d = if room.depth_m > MIN_ROOM_M {
        room.depth_m
    } else {
        DEFAULT_ROOM_D
    };
    let room_h = if room.height_m > MIN_ROOM_M {
        room.height_m
    } else {
        DEFAULT_ROOM_H
    };

    for object in objects {
        let depth = object_depth(object, room_d);
        let lateral = (object.x.clamp(0.0, 1.0) - 0.5) * 2.0 * depth * HALF_FOV_TAN;

        let vx = centered_voxel(lateral, room_w, VOXEL_W);
        let vy = forward_voxel(OBJECT_HEIGHT_M, room_h, VOXEL_H);
        let vz = forward_voxel(depth, room_d, VOXEL_D);
        let occupancy =
            ((object.confidence.clamp(0.0, 1.0) * OCCUPANCY_SCALE) as u8).saturating_add(OCCUPANCY_BASE);

        stamp_voxel(voxels, vx, vy, vz, occupancy, name_color(&object.name));
    }

    voxels.update_seq.fetch_add(1, Ordering::Release);
}

/// Writes the scene embedding into the senses layer so layer 0 has a live
/// prediction target.
fn inject_to_senses_layer(shm: &ShmRegion, world: &WorldModel) {
    let slot = shm.layer_slot(NUM_LAYERS - 1);
    let writer = LayerWriter::new(slot);
    let buffer = writer.back_buffer();
    buffer.layer = (NUM_LAYERS - 1) as u8;
    let mut energy = 0.0f32;
    for dim in 0..STATE_DIM {
        let value = world.scene_embedding[dim];
        buffer.mean[dim] = value;
        buffer.precision[dim] = PRECISION_FLOOR + value.abs() * PRECISION_GAIN;
        energy += value * value;
    }
    let vfe = energy / STATE_DIM as f32;
    buffer.vfe = vfe;
    buffer.challenge_vfe = vfe;
    writer.publish();
}

/// Stamps the world model as freshly observed.
fn note_frame(shm: &ShmRegion) {
    let world = shm.world_model_mut();
    world.last_vision_ns = now_ns();
    world.vision_frame_count += 1;
}

/// Writes the starting directive.
fn set_default_directive(world: &mut WorldModel) {
    write_cstr(&mut world.directive, DEFAULT_DIRECTIVE);
}

/// Wall-clock nanoseconds since the Unix epoch, or zero before it.
fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// One online pass
// ---------------------------------------------------------------------------

/// Runs one complete online pass over `jpeg`: harvest questions, call the
/// vision model, project the scene into the world and voxel grid, embed the
/// scene text, and record any lore. `call_number` is the one-based index of
/// this call, as the reference numbers its thoughts and diagnostics.
///
/// A vision failure mutates nothing and returns the error; a failed embedding
/// falls back to [`hash_embedding`] and still counts as a pass.
fn run_vision_tick(
    shm: &ShmRegion,
    transport: &impl Transport,
    api_key: &str,
    jpeg: &[u8],
    call_number: u64,
) -> Result<(), String> {
    let image_b64 = base64::engine::general_purpose::STANDARD.encode(jpeg);
    let directive = read_cstr(&shm.world_model().directive);
    let before: [f32; STATE_DIM] = shm.world_model().scene_embedding;
    let questions = harvest_questions(shm);

    eprintln!(
        "qualia-vision: calling Gemini Vision API ({}B image)...",
        image_b64.len()
    );
    let (response, usage) = call_vision(transport, api_key, &image_b64, &directive, &questions)?;

    {
        let world = shm.world_model_mut();
        world.gemini_input_tokens += usage.input_tokens;
        world.gemini_output_tokens += usage.output_tokens;
    }

    let labels: Vec<&str> = response.objects.iter().map(|o| o.name.as_str()).collect();
    shm.emit_thought(
        RUNNER_LAYER,
        THOUGHT_OBSERVE,
        0.0,
        &format!(
            "gemini vision #{}: {} obj={} [{}]",
            call_number,
            excerpt(&response.scene, SCENE_EXCERPT),
            response.objects.len(),
            labels.join(",")
        ),
    );

    {
        let world = shm.world_model_mut();
        apply_vision_response(world, &response);
    }
    {
        let voxels = shm.world_voxels_mut();
        update_world_voxels(voxels, &response.objects, &response.room);
    }

    let embedding_text = format!("{} {} {}", response.scene, response.activity, labels.join(" "));
    match call_embedding(transport, api_key, &embedding_text) {
        Ok((embedding, tokens)) => {
            {
                let world = shm.world_model_mut();
                world.gemini_embedding_tokens += tokens;
                project_embedding_to_scene(world, &embedding);
            }
            let norm: f32 = embedding.iter().map(|value| value * value).sum::<f32>().sqrt();
            shm.emit_thought(
                RUNNER_LAYER,
                THOUGHT_LEARN,
                0.0,
                &format!(
                    "embedding: {}d, norm={:.3}, projected to 64d",
                    embedding.len(),
                    norm
                ),
            );
        }
        Err(error) => {
            eprintln!("qualia-vision: embedding API error: {error}");
            let world = shm.world_model_mut();
            hash_embedding(world, &response);
        }
    }

    let delta = embedding_delta(&before, &shm.world_model().scene_embedding);
    record_lore(shm, &questions, &response.lore_answers, delta);

    {
        let world = shm.world_model_mut();
        world.last_llm_ns = now_ns();
        world.llm_call_count += 1;
        world.update_seq.fetch_add(1, Ordering::Release);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Offline scene synthesis
// ---------------------------------------------------------------------------

/// Cheap statistics over the sensed belief vector.
#[derive(Clone, Copy, Debug, PartialEq)]
struct SensorStats {
    brightness: f32,
    variance: f32,
    edge_energy: f32,
}

/// Summarises a belief vector into brightness, variance and edge energy.
fn sensor_stats(mean: &[f32; STATE_DIM]) -> SensorStats {
    let brightness = mean.iter().sum::<f32>() / STATE_DIM as f32;
    let variance = mean
        .iter()
        .map(|value| (value - brightness).powi(2))
        .sum::<f32>()
        / STATE_DIM as f32;
    let edge_energy = mean.windows(2).map(|pair| (pair[1] - pair[0]).abs()).sum::<f32>()
        / (STATE_DIM - 1) as f32;
    SensorStats {
        brightness,
        variance,
        edge_energy,
    }
}

/// The synthetic scene the offline loop publishes from sensor statistics.
fn offline_response(stats: &SensorStats, tick: u64) -> VisionResponse {
    let sweep = (tick as f32 * OFFLINE_SWEEP_RATE).sin();
    let objects = vec![
        VisionObject {
            name: "anchor".to_string(),
            confidence: 0.95,
            x: (0.35 + sweep * 0.08).clamp(0.1, 0.9),
            y: 0.58,
            depth_m: 1.6,
        },
        VisionObject {
            name: "pillar".to_string(),
            confidence: 0.88,
            x: (0.68 - sweep * 0.06).clamp(0.1, 0.9),
            y: 0.46,
            depth_m: 2.8,
        },
    ];
    let activity = if stats.variance > MOTION_VARIANCE {
        "local motion estimate"
    } else if stats.brightness > LIT_BRIGHTNESS {
        "local lit scene"
    } else {
        "local idle scene"
    };
    VisionResponse {
        scene: format!(
            "local voxel map: bright={:.2} var={:.3} edge={:.3}",
            stats.brightness, stats.variance, stats.edge_energy
        ),
        activity: activity.to_string(),
        objects,
        lore_answers: Vec::new(),
        room: Room {
            width_m: DEFAULT_ROOM_W,
            depth_m: DEFAULT_ROOM_D,
            height_m: DEFAULT_ROOM_H,
        },
    }
}

/// Fills the scene embedding from the sensed belief and the pass counter.
fn offline_embedding(world: &mut WorldModel, sensor: &BeliefSlot, stats: &SensorStats, tick: u64) {
    for (dim, value) in world.scene_embedding.iter_mut().enumerate() {
        *value = match dim % 8 {
            0 => stats.brightness,
            1 => stats.variance,
            2 => stats.edge_energy,
            3 => sensor.mean[dim],
            4 => (stats.brightness - 0.5).abs(),
            5 => {
                if stats.variance > EMBED_FLAG_VARIANCE {
                    1.0
                } else {
                    0.0
                }
            }
            6 => sensor.precision[dim] * 0.01,
            _ => (tick as f32 * 0.001).sin() * 0.1,
        };
    }
}

/// The sensor thought one offline pass owes, if any: its VFE and its text.
///
/// A pass whose brightness moved, whose object count changed, or whose scene
/// is moving reports itself; on the otherwise quiet passes every
/// [`OFFLINE_FALLBACK_INTERVAL`] a still note stands in for it.
fn offline_sensor_thought(
    tick: u64,
    stats: &SensorStats,
    objects: u32,
    previous_objects: u32,
    previous_brightness: f32,
) -> Option<(f32, String)> {
    if tick % OFFLINE_THOUGHT_INTERVAL != 0 {
        return None;
    }
    let brightness_change = (stats.brightness - previous_brightness).abs();
    if brightness_change > SENSOR_DELTA
        || objects != previous_objects
        || stats.variance > MOTION_VARIANCE
    {
        return Some((
            brightness_change,
            format!(
                "sensor: bright={:.2} var={:.3} edge={:.3} obj={} delta_b={:.2}",
                stats.brightness, stats.variance, stats.edge_energy, objects, brightness_change
            ),
        ));
    }
    if tick % OFFLINE_FALLBACK_INTERVAL == 0 {
        // The argument order of this line is fixed by the emitted contract.
        return Some((
            0.0,
            format!(
                "sensor: bright={:.2} var={:.3} obj={} tick={}",
                stats.brightness, objects, stats.variance, tick
            ),
        ));
    }
    None
}

/// Captures one JPEG frame from the best source the platform offers.
///
/// On Linux the camera is owned by the agent process, so a snapshot file is
/// read instead of opening the device a second time. Elsewhere `ffmpeg` grabs
/// one frame from the default capture device.
fn capture_frame() -> Result<Vec<u8>, String> {
    #[cfg(target_os = "linux")]
    {
        for snapshot in &["/tmp/qualia_orin_snap.jpg", "/tmp/qualia_snapshot.jpg"] {
            if let Ok(data) = std::fs::read(snapshot) {
                if !data.is_empty() {
                    eprintln!("qualia-vision: using snapshot {snapshot} ({}B)", data.len());
                    return Ok(data);
                }
            }
        }
        Err("no snapshot available — qualia-agent must be running to capture /dev/video0, \
             or post a JPEG to /snapshot from camera.html"
            .to_string())
    }

    #[cfg(not(target_os = "linux"))]
    {
        eprintln!("qualia-vision: calling ffmpeg...");
        let video_size = format!("{CAPTURE_WIDTH}x{CAPTURE_HEIGHT}");
        let output = std::process::Command::new("ffmpeg")
            .args(["-y", "-hide_banner", "-loglevel", "error"])
            .args([
                "-f",
                "avfoundation",
                "-framerate",
                "30",
                "-video_size",
                video_size.as_str(),
                "-i",
                "0",
            ])
            .args([
                "-frames:v",
                "1",
                "-f",
                "image2",
                "-c:v",
                "mjpeg",
                "-q:v",
                "5",
                "pipe:1",
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .map_err(|error| format!("ffmpeg: {error}"))?;

        eprintln!(
            "qualia-vision: ffmpeg returned, status={}, stdout={}B",
            output.status,
            output.stdout.len()
        );

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!("qualia-vision: ffmpeg stderr: {}", stderr);
            return Err(format!("ffmpeg failed: {stderr}"));
        }
        if output.stdout.is_empty() {
            return Err("ffmpeg produced no output".to_string());
        }
        Ok(output.stdout)
    }
}

// ---------------------------------------------------------------------------
// Loops
// ---------------------------------------------------------------------------

/// Offline mode: synthesise a scene from sensor statistics without a model.
fn run_offline_loop(shm: &ShmRegion) {
    set_default_directive(shm.world_model_mut());
    shm.emit_thought(
        RUNNER_LAYER,
        THOUGHT_OBSERVE,
        0.0,
        "vision: offline mode, no GEMINI_API_KEY",
    );

    let mut tick: u64 = 0;
    let mut previous_brightness = 0.0f32;
    let mut previous_objects = 0u32;
    loop {
        let sensor = {
            let slot = shm.layer_slot(NUM_LAYERS - 1);
            let reader = LayerReader::new(slot);
            *reader.read()
        };
        let stats = sensor_stats(&sensor.mean);
        let response = offline_response(&stats, tick);

        {
            let world = shm.world_model_mut();
            apply_vision_response(world, &response);
            offline_embedding(world, &sensor, &stats, tick);
        }
        {
            let voxels = shm.world_voxels_mut();
            update_world_voxels(voxels, &response.objects, &response.room);
        }
        note_frame(shm);
        shm.world_model().update_seq.fetch_add(1, Ordering::Release);

        tick += 1;
        let objects = shm.world_model().num_objects;
        if let Some((vfe, text)) =
            offline_sensor_thought(tick, &stats, objects, previous_objects, previous_brightness)
        {
            shm.emit_thought(RUNNER_LAYER, THOUGHT_OBSERVE, vfe, &text);
        }
        previous_brightness = stats.brightness;
        previous_objects = objects;

        if tick % OFFLINE_STDERR_INTERVAL == 0 {
            eprintln!(
                "qualia-vision: offline tick {}, {} objects, brightness={:.2}",
                tick, objects, stats.brightness
            );
        }

        std::thread::sleep(Duration::from_millis(LOOP_PERIOD_MS));
    }
}

/// Online mode: capture, understand and embed on a fixed interval until the
/// call budget runs out, then keep the sense signal alive from the sensors.
fn run_vision_loop(shm: &ShmRegion, transport: &impl Transport, config: &Config) {
    set_default_directive(shm.world_model_mut());
    shm.emit_thought(
        RUNNER_LAYER,
        THOUGHT_OBSERVE,
        0.0,
        &format!(
            "vision: gemini online, budget={}, interval={}s",
            config.llm_max_calls, config.llm_interval_secs
        ),
    );

    let interval_secs = config.llm_interval_secs;
    let mut last_call = Instant::now()
        .checked_sub(Duration::from_secs(interval_secs + 1))
        .unwrap_or_else(Instant::now);
    let mut calls: u64 = 0;
    let mut budget_spent = false;

    loop {
        let elapsed = last_call.elapsed().as_secs();
        if !budget_spent && elapsed >= interval_secs {
            eprintln!("qualia-vision: triggering capture ({}s since last)", elapsed);
            last_call = Instant::now();
            let call_number = calls + 1;
            match capture_frame() {
                Ok(jpeg) => {
                    match run_vision_tick(shm, transport, &config.api_key, &jpeg, call_number) {
                        Ok(()) => {
                            calls = call_number;
                            let objects = shm.world_model().num_objects;
                            eprintln!(
                                "qualia-vision: Gemini call #{}/{} — {} objects",
                                calls, config.llm_max_calls, objects
                            );
                            if calls >= config.llm_max_calls {
                                budget_spent = true;
                                shm.emit_thought(
                                    RUNNER_LAYER,
                                    THOUGHT_ESCALATE,
                                    0.0,
                                    &format!(
                                        "budget exhausted: {}/{} calls, sensor-only mode",
                                        calls, config.llm_max_calls
                                    ),
                                );
                                eprintln!(
                                    "qualia-vision: Gemini budget exhausted ({}/{}). Continuing with sensor data only.",
                                    calls, config.llm_max_calls
                                );
                            }
                        }
                        Err(error) => {
                            eprintln!("qualia-vision: Gemini vision error: {error}");
                            shm.emit_thought(
                                RUNNER_LAYER,
                                THOUGHT_ESCALATE,
                                0.0,
                                &format!("vision err: {}", error),
                            );
                        }
                    }
                }
                Err(error) => {
                    eprintln!("qualia-vision: capture error: {error}");
                    shm.emit_thought(
                        RUNNER_LAYER,
                        THOUGHT_ESCALATE,
                        0.0,
                        &format!("capture err: {}", error),
                    );
                }
            }
        }

        note_frame(shm);
        inject_to_senses_layer(shm, shm.world_model());

        std::thread::sleep(Duration::from_millis(LOOP_PERIOD_MS));
    }
}

fn main() {
    let config = Config::from_env();
    eprintln!("qualia-vision: opening shm '{}'", config.shm_name);
    let shm = ShmRegion::open(&config.shm_name)
        .unwrap_or_else(|error| panic!("qualia-vision: failed to open shm: {error}"));

    if config.online() {
        eprintln!("qualia-vision: Gemini API enabled (vision + embeddings)");
        eprintln!(
            "qualia-vision: Interval: {}s, Max calls: {}",
            config.llm_interval_secs, config.llm_max_calls
        );
        run_vision_loop(&shm, &HttpTransport, &config);
    } else {
        eprintln!("qualia-vision: WARNING: GEMINI_API_KEY not set");
        eprintln!("qualia-vision: Running in offline mode — synthetic world model only");
        run_offline_loop(&shm);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use qualia_shm::{LAYER_SLOTS_OFFSET, LAYER_SLOT_SIZE};
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::sync::atomic::AtomicU64;

    static NEXT_REGION: AtomicU64 = AtomicU64::new(0);

    fn test_region(tag: &str) -> ShmRegion {
        let index = NEXT_REGION.fetch_add(1, Ordering::Relaxed);
        let name = format!(
            "/qualia_vision_test_{}_{}_{}",
            std::process::id(),
            tag,
            index
        );
        ShmRegion::create(&name).expect("create test region")
    }

    /// A transport that hands out scripted bodies and remembers every request.
    struct FakeTransport {
        replies: RefCell<VecDeque<Result<String, String>>>,
        requests: RefCell<Vec<(String, String)>>,
    }

    impl FakeTransport {
        fn scripted(replies: Vec<Result<String, String>>) -> Self {
            Self {
                replies: RefCell::new(replies.into()),
                requests: RefCell::new(Vec::new()),
            }
        }

        fn last_request(&self) -> (String, String) {
            self.requests.borrow().last().cloned().expect("a request")
        }
    }

    impl Transport for FakeTransport {
        fn post_json(&self, url: &str, json: &str) -> Result<String, String> {
            self.requests
                .borrow_mut()
                .push((url.to_string(), json.to_string()));
            self.replies
                .borrow_mut()
                .pop_front()
                .unwrap_or_else(|| Err("fake transport ran out of replies".to_string()))
        }
    }

    const SCENE_JSON: &str = r#"{
        "scene": "a mug sits on a desk by a window",
        "activity": "the room is still",
        "room": {"width_m": 5.0, "depth_m": 4.0, "height_m": 2.4},
        "objects": [{"name": "mug", "confidence": 0.9, "x": 0.25, "y": 0.7, "depth_m": 1.5}],
        "lore_answers": ["a mug"]
    }"#;

    fn vision_envelope(text: &str) -> String {
        serde_json::json!({
            "candidates": [{"content": {"parts": [{"text": text}]}}],
            "usageMetadata": {"promptTokenCount": 12, "candidatesTokenCount": 34}
        })
        .to_string()
    }

    fn embedding_envelope(dim: usize, value: f32, tokens: u64) -> String {
        embedding_values(vec![value; dim], tokens)
    }

    fn embedding_values(values: Vec<f32>, tokens: u64) -> String {
        serde_json::json!({
            "embedding": {"values": values},
            "usageMetadata": {"totalTokenCount": tokens}
        })
        .to_string()
    }

    fn raise_question(shm: &ShmRegion, layer: usize, reason: u8, text: &str) {
        // The arena exposes layer slots read-only to every runner but a layer's
        // own writer; a test stands in for that writer through the documented
        // slot offsets.
        //
        // SAFETY: `layer` is in range, the slot lies inside the mapped arena,
        // and this test is the only thread touching the region.
        let slot = unsafe {
            &mut *shm
                .as_ptr()
                .add(LAYER_SLOTS_OFFSET + layer * LAYER_SLOT_SIZE)
                .cast::<LayerSlot>()
        };
        slot.question.text = [0u8; MAX_QUESTION_TEXT];
        write_cstr(&mut slot.question.text, text);
        slot.question.layer = layer as u8;
        slot.question.reason = reason;
        slot.question.pending.store(true, Ordering::Release);
    }

    /// Every thought the region holds, oldest first, as (kind, text).
    fn thought_texts(shm: &ShmRegion) -> Vec<(u8, String)> {
        let buffer = shm.thought_buffer();
        let written = buffer.write_seq.load(Ordering::Acquire);
        (0..written)
            .map(|seq| {
                let entry = &buffer.entries[(seq as usize) % MAX_THOUGHTS];
                assert_eq!(entry.seq, seq, "thought ring slot for seq {seq}");
                (entry.kind, read_cstr(&entry.text))
            })
            .collect()
    }

    #[test]
    fn config_reads_the_documented_keys_and_defaults_the_rest() {
        let configured = Config::from_lookup(|key| match key {
            "QUALIA_SHM_NAME" => Some("/qualia_test".to_string()),
            "GEMINI_API_KEY" => Some("secret".to_string()),
            "QUALIA_LLM_INTERVAL" => Some("7".to_string()),
            "QUALIA_LLM_MAX_CALLS" => Some("3".to_string()),
            _ => None,
        });
        assert_eq!(configured.shm_name, "/qualia_test");
        assert_eq!(configured.api_key, "secret");
        assert_eq!(configured.llm_interval_secs, 7);
        assert_eq!(configured.llm_max_calls, 3);
        assert!(configured.online());

        let defaulted = Config::from_lookup(|_| None);
        assert_eq!(defaulted.shm_name, DEFAULT_SHM_NAME);
        assert!(defaulted.api_key.is_empty());
        assert_eq!(defaulted.llm_interval_secs, DEFAULT_LLM_INTERVAL_SECS);
        assert_eq!(defaulted.llm_max_calls, DEFAULT_LLM_MAX_CALLS);
        assert!(!defaulted.online());

        let malformed = Config::from_lookup(|key| match key {
            "QUALIA_LLM_INTERVAL" => Some("soon".to_string()),
            "QUALIA_LLM_MAX_CALLS" => Some(String::new()),
            _ => None,
        });
        assert_eq!(malformed.llm_interval_secs, DEFAULT_LLM_INTERVAL_SECS);
        assert_eq!(malformed.llm_max_calls, DEFAULT_LLM_MAX_CALLS);
    }

    #[test]
    fn vision_call_parses_the_scene_schema_and_token_counts() {
        let transport = FakeTransport::scripted(vec![Ok(vision_envelope(SCENE_JSON))]);
        let (response, usage) = call_vision(&transport, "k123", "aW1hZ2U=", "look around", &[])
            .expect("vision call");

        assert_eq!(response.scene, "a mug sits on a desk by a window");
        assert_eq!(response.activity, "the room is still");
        assert_eq!(response.objects.len(), 1);
        assert_eq!(response.objects[0].name, "mug");
        assert_eq!(response.objects[0].confidence, 0.9);
        assert_eq!(response.room.width_m, 5.0);
        assert_eq!(response.room.height_m, 2.4);
        assert_eq!(response.lore_answers, vec!["a mug".to_string()]);
        assert_eq!(usage.input_tokens, 12);
        assert_eq!(usage.output_tokens, 34);

        let (url, body) = transport.last_request();
        assert!(url.contains("/gemini-2.5-flash:generateContent"));
        assert!(url.ends_with("key=k123"));
        assert!(body.contains(r#""inline_data""#));
        assert!(body.contains(r#""mime_type":"image/jpeg""#));
        assert!(body.contains(r#""data":"aW1hZ2U=""#));
        assert!(body.contains(r#""maxOutputTokens":2048"#));
        assert!(body.contains("look around"));
        assert!(!body.contains("lore_answers"));
    }

    #[test]
    fn a_fenced_payload_still_parses_and_prose_around_it_does_not() {
        let fenced = format!("```json\n{SCENE_JSON}\n```");
        let transport = FakeTransport::scripted(vec![Ok(vision_envelope(&fenced))]);
        let (response, _) = call_vision(&transport, "k", "a", "d", &[]).expect("fenced parse");
        assert_eq!(response.objects[0].name, "mug");

        // Only a bare fence is stripped; prose ahead of it is left in place.
        let wrapped = format!("Here it is:\n```json\n{SCENE_JSON}\n```");
        let transport = FakeTransport::scripted(vec![Ok(vision_envelope(&wrapped))]);
        assert!(call_vision(&transport, "k", "a", "d", &[]).is_err());
    }

    #[test]
    fn harvested_questions_are_appended_to_the_prompt() {
        let questions = vec![
            PendingQuestion {
                layer: 2,
                reason: 0,
                text: "what is that?".to_string(),
            },
            PendingQuestion {
                layer: 5,
                reason: 2,
                text: "have I seen this?".to_string(),
            },
        ];
        let prompt = vision_prompt("stay put", &questions);
        assert!(prompt.contains("\"lore_answers\""));
        assert!(prompt.contains("<answer to layer 2: what is that?>"));
        assert!(prompt.contains("<answer to layer 5: have I seen this?>"));
        assert!(prompt.contains("2 question(s)"));
    }

    #[test]
    fn vision_tick_writes_the_scene_voxels_and_accounting() {
        let shm = test_region("tick");
        let transport = FakeTransport::scripted(vec![
            Ok(vision_envelope(SCENE_JSON)),
            Ok(embedding_envelope(STATE_DIM, 0.5, 7)),
        ]);

        run_vision_tick(&shm, &transport, "key", b"jpeg-bytes", 1).expect("tick");

        let world = shm.world_model();
        assert_eq!(read_cstr(&world.scene), "a mug sits on a desk by a window");
        assert_eq!(read_cstr(&world.activity), "the room is still");
        assert_eq!(world.num_objects, 1);
        assert_eq!(read_cstr(&world.objects[0].name), "mug");
        assert_eq!(world.objects[0].active, 1);
        assert_eq!(world.llm_call_count, 1);
        assert_eq!(world.gemini_input_tokens, 12);
        assert_eq!(world.gemini_output_tokens, 34);
        assert_eq!(world.gemini_embedding_tokens, 7);
        assert!(world.scene_embedding.iter().all(|value| *value == 0.5));
        assert!(world.update_seq.load(Ordering::Acquire) >= 1);

        let voxels = shm.world_voxels();
        assert!(voxels.cells.iter().any(|cell| cell.occupancy > 0));
        assert!(voxels.update_seq.load(Ordering::Acquire) >= 1);

        let thoughts = thought_texts(&shm);
        assert_eq!(thoughts.len(), 2);
        assert_eq!(thoughts[0].0, THOUGHT_OBSERVE);
        assert_eq!(
            thoughts[0].1,
            "gemini vision #1: a mug sits on a desk by a window obj=1 [mug]"
        );
        assert_eq!(thoughts[1].0, THOUGHT_LEARN);
        assert_eq!(
            thoughts[1].1,
            "embedding: 1024d, norm=16.000, projected to 64d"
        );

        let (url, body) = transport.last_request();
        assert!(url.contains("/gemini-embedding-2-preview:embedContent"));
        assert!(body.contains(r#""outputDimensionality":1024"#));
        assert!(body.contains("a mug sits on a desk by a window"));
    }

    #[test]
    fn embedding_is_fitted_to_the_scene_width() {
        let shm = test_region("project");
        let pooled = [vec![1.0f32; STATE_DIM], vec![-1.0f32; STATE_DIM]].concat();
        let transport = FakeTransport::scripted(vec![
            Ok(vision_envelope(SCENE_JSON)),
            Ok(embedding_values(vec![0.75; STATE_DIM], 3)),
            Ok(vision_envelope(SCENE_JSON)),
            Ok(embedding_values(pooled, 3)),
            Ok(vision_envelope(SCENE_JSON)),
            Ok(embedding_envelope(8, 0.75, 3)),
        ]);

        // Exactly STATE_DIM values are copied straight through.
        run_vision_tick(&shm, &transport, "key", b"jpeg", 1).expect("tick");
        assert!(shm
            .world_model()
            .scene_embedding
            .iter()
            .all(|value| (*value - 0.75).abs() < 1e-6));

        // A wider embedding is average-pooled into STATE_DIM bins.
        run_vision_tick(&shm, &transport, "key", b"jpeg", 1).expect("tick");
        let pooled_embedding = shm.world_model().scene_embedding;
        assert!(pooled_embedding[..STATE_DIM / 2]
            .iter()
            .all(|value| (*value - 1.0).abs() < 1e-6));
        assert!(pooled_embedding[STATE_DIM / 2..]
            .iter()
            .all(|value| (*value + 1.0).abs() < 1e-6));

        // A narrower embedding fills the front and zero-pads the tail.
        run_vision_tick(&shm, &transport, "key", b"jpeg", 1).expect("tick");
        let narrow = shm.world_model().scene_embedding;
        assert!(narrow[..8].iter().all(|value| (*value - 0.75).abs() < 1e-6));
        assert!(narrow[8..].iter().all(|value| *value == 0.0));
    }

    #[test]
    fn vision_failure_reports_the_error_and_leaves_the_world_untouched() {
        let shm = test_region("failure");
        let transport = FakeTransport::scripted(vec![Err("connection refused".to_string())]);
        let error = run_vision_tick(&shm, &transport, "key", b"jpeg", 1).expect_err("must fail");
        assert!(error.contains("connection refused"), "got {error}");

        let world = shm.world_model();
        assert_eq!(world.llm_call_count, 0);
        assert_eq!(world.num_objects, 0);
        assert!(world.scene_embedding.iter().all(|value| *value == 0.0));
        assert!(!shm
            .world_voxels()
            .cells
            .iter()
            .any(|cell| cell.occupancy > 0));
        assert!(thought_texts(&shm).is_empty());
    }

    #[test]
    fn a_body_that_is_not_the_scene_schema_is_an_error() {
        let shm = test_region("badbody");
        let transport = FakeTransport::scripted(vec![Ok("<html>502 bad gateway</html>".to_string())]);
        let error = run_vision_tick(&shm, &transport, "key", b"jpeg", 1).expect_err("must fail");
        assert!(error.contains("not JSON"), "got {error}");
        assert_eq!(shm.world_model().llm_call_count, 0);
    }

    #[test]
    fn failed_embedding_falls_back_to_the_local_hash() {
        let shm = test_region("fallback");
        let transport = FakeTransport::scripted(vec![
            Ok(vision_envelope(SCENE_JSON)),
            Err("embedding service down".to_string()),
        ]);
        run_vision_tick(&shm, &transport, "key", b"jpeg", 1).expect("tick");

        let world = shm.world_model();
        assert_eq!(world.gemini_embedding_tokens, 0);
        assert_eq!(world.num_objects, 1);
        assert!(world
            .scene_embedding
            .iter()
            .any(|value| value.is_finite() && *value != 0.0));

        // A failed embedding is not reported as a thought, only the vision is.
        let thoughts = thought_texts(&shm);
        assert_eq!(thoughts.len(), 1);
        assert_eq!(thoughts[0].0, THOUGHT_OBSERVE);
    }

    #[test]
    fn pending_questions_are_harvested_once_and_cleared() {
        let shm = test_region("questions");
        raise_question(&shm, 2, 0, "what is that?");
        raise_question(&shm, 5, 2, "have I seen this?");

        let harvested = harvest_questions(&shm);
        assert_eq!(
            harvested,
            vec![
                PendingQuestion {
                    layer: 2,
                    reason: 0,
                    text: "what is that?".to_string()
                },
                PendingQuestion {
                    layer: 5,
                    reason: 2,
                    text: "have I seen this?".to_string()
                }
            ]
        );
        assert!(!shm.layer_slot(2).question.pending.load(Ordering::Acquire));
        assert!(!shm.layer_slot(5).question.pending.load(Ordering::Acquire));
        assert!(harvest_questions(&shm).is_empty());

        shm.layer_slot(1)
            .question
            .pending
            .store(true, Ordering::Release);
        assert!(harvest_questions(&shm).is_empty());
        assert!(!shm.layer_slot(1).question.pending.load(Ordering::Acquire));
    }

    #[test]
    fn answered_questions_become_lore_entries() {
        let shm = test_region("lore");
        raise_question(&shm, 2, 0, "what is that?");
        raise_question(&shm, 5, 2, "have I seen this?");

        let transport = FakeTransport::scripted(vec![
            Ok(vision_envelope(SCENE_JSON)),
            Ok(embedding_envelope(STATE_DIM, 0.5, 7)),
        ]);
        run_vision_tick(&shm, &transport, "key", b"jpeg", 1).expect("tick");

        let lore = shm.lore_buffer();
        assert_eq!(lore.write_seq.load(Ordering::Acquire), 1);
        let entry = &lore.entries[0];
        assert_eq!(read_cstr(&entry.question), "what is that?");
        assert_eq!(read_cstr(&entry.answer), "a mug");
        assert_eq!(entry.layer, 2);
        assert_eq!(entry.reason, 0);
        assert!(entry.embedding_delta > 0.0);

        let thoughts = thought_texts(&shm);
        assert_eq!(thoughts.len(), 3);
        assert_eq!(thoughts[2].0, THOUGHT_LEARN);
        assert_eq!(thoughts[2].1, "lore L2 r=0: Q=what is that? A=a mug");

        let (_, body) = transport.last_request();
        assert!(body.contains("a mug sits on a desk by a window"));
    }

    #[test]
    fn offline_scene_tracks_the_sensor_statistics() {
        let bright = [0.6f32; STATE_DIM];
        let stats = sensor_stats(&bright);
        assert!((stats.brightness - 0.6).abs() < 1e-6);
        assert!(stats.variance < 1e-6);

        let response = offline_response(&stats, 0);
        assert_eq!(response.objects.len(), 2);
        assert_eq!(response.objects[0].name, "anchor");
        assert_eq!(response.objects[1].name, "pillar");
        assert!(response.scene.contains("bright=0.60"));
        assert_eq!(response.activity, "local lit scene");

        let moving = SensorStats {
            brightness: 0.2,
            variance: 0.5,
            edge_energy: 0.1,
        };
        assert_eq!(offline_response(&moving, 0).activity, "local motion estimate");

        let idle = SensorStats {
            brightness: 0.2,
            variance: 0.0,
            edge_energy: 0.1,
        };
        assert_eq!(offline_response(&idle, 0).activity, "local idle scene");
    }

    #[test]
    fn offline_embedding_follows_the_sensed_belief() {
        let shm = test_region("offline");
        {
            let slot = shm.layer_slot(NUM_LAYERS - 1);
            let writer = LayerWriter::new(slot);
            let buffer = writer.back_buffer();
            for dim in 0..STATE_DIM {
                buffer.mean[dim] = if dim % 2 == 0 { 0.0 } else { 1.0 };
                buffer.precision[dim] = 0.5;
            }
            writer.publish();
        }
        let sensor = *LayerReader::new(shm.layer_slot(NUM_LAYERS - 1)).read();
        let stats = sensor_stats(&sensor.mean);
        assert!((stats.brightness - 0.5).abs() < 1e-6);
        assert!((stats.variance - 0.25).abs() < 1e-6);

        offline_embedding(shm.world_model_mut(), &sensor, &stats, 3);
        let embedding = shm.world_model().scene_embedding;
        assert_eq!(embedding[0], stats.brightness);
        assert_eq!(embedding[1], stats.variance);
        assert_eq!(embedding[2], stats.edge_energy);
        assert_eq!(embedding[3], sensor.mean[3]);
        assert_eq!(embedding[4], (stats.brightness - 0.5).abs());
        assert_eq!(embedding[5], 1.0);
        assert_eq!(embedding[6], sensor.precision[6] * 0.01);
        assert_eq!(embedding[7], (3.0f32 * 0.001).sin() * 0.1);
    }

    #[test]
    fn offline_sensor_thought_reports_changes_and_keeps_quiet() {
        let stats = SensorStats {
            brightness: 0.6,
            variance: 0.0,
            edge_energy: 0.02,
        };

        // Off-cadence passes say nothing.
        assert_eq!(offline_sensor_thought(29, &stats, 2, 2, 0.6), None);
        // An unchanged pass at the cadence says nothing either.
        assert_eq!(offline_sensor_thought(30, &stats, 2, 2, 0.6), None);
        // A brighter pass reports itself, with the change as its VFE.
        let (vfe, text) = offline_sensor_thought(30, &stats, 2, 2, 0.4).expect("a thought");
        assert!((vfe - 0.2).abs() < 1e-6);
        assert_eq!(
            text,
            "sensor: bright=0.60 var=0.000 edge=0.020 obj=2 delta_b=0.20"
        );
        // A new object reports itself even when the brightness held.
        assert!(offline_sensor_thought(60, &stats, 3, 2, 0.6).is_some());
        // The quiet fallback stands in every 150th pass.
        let (vfe, text) = offline_sensor_thought(150, &stats, 2, 2, 0.6).expect("a fallback");
        assert_eq!(vfe, 0.0);
        assert_eq!(text, "sensor: bright=0.60 var=2 obj=0 tick=150");
    }

    #[test]
    fn directive_defaults_to_the_reference_text() {
        let shm = test_region("directive");
        set_default_directive(shm.world_model_mut());
        assert_eq!(
            read_cstr(&shm.world_model().directive),
            "Observe and understand the environment. Report what you see."
        );
    }

    #[test]
    fn voxel_occupancy_is_projected_and_decays() {
        let shm = test_region("voxels");
        let room = Room {
            width_m: 6.0,
            depth_m: 6.0,
            height_m: 2.5,
        };
        let objects = vec![VisionObject {
            name: "mug".to_string(),
            confidence: 0.9,
            x: 0.5,
            y: 0.5,
            depth_m: 1.0,
        }];

        update_world_voxels(shm.world_voxels_mut(), &objects, &room);
        let peak = shm
            .world_voxels()
            .cells
            .iter()
            .map(|cell| cell.occupancy)
            .max()
            .unwrap();
        assert_eq!(peak, 55 + (0.9f32 * 200.0) as u8);
        assert_eq!(shm.world_voxels().update_seq.load(Ordering::Acquire), 1);

        // An object with no depth estimate is placed from its frame position.
        let estimated = vec![VisionObject {
            name: "box".to_string(),
            confidence: 1.0,
            x: 0.1,
            y: 0.2,
            depth_m: 0.0,
        }];
        update_world_voxels(shm.world_voxels_mut(), &estimated, &room);
        let occupied_after = shm
            .world_voxels()
            .cells
            .iter()
            .filter(|cell| cell.occupancy > 0)
            .count();
        assert!(occupied_after > 0);
        assert_eq!(shm.world_voxels().update_seq.load(Ordering::Acquire), 2);

        // With nothing detected the paint decays but never underflows.
        let before_decay = shm
            .world_voxels()
            .cells
            .iter()
            .map(|cell| cell.occupancy)
            .max()
            .unwrap();
        update_world_voxels(shm.world_voxels_mut(), &[], &room);
        let after_decay = shm
            .world_voxels()
            .cells
            .iter()
            .map(|cell| cell.occupancy)
            .max()
            .unwrap();
        assert_eq!(after_decay, before_decay - VOXEL_DECAY);
    }
}
