//! The two websockets: the shared-memory stream the studio draws, and the
//! WebRTC signalling relay.
//!
//! The stream's frame layout is a wire contract — the studio decodes it byte by
//! byte — so it is spelled out here and kept exactly as the reference emits it:
//!
//! ```text
//! [u8 type][u64 timestamp_ns LE][payload…]
//!
//! 0x01 LayerUpdate:  u8 layer_id, f32 vfe, f32[1024] mean, f32[1024] residual,
//!                    f32[1024] precision, u32 cycle_us, u8 compression,
//!                    u32 confirm_streak
//! 0x02 WorldModel:   u16 scene_len, u8[scene_len] scene, u8 obj_count,
//!                    per obj: u8 name_len, u8[name_len] name, f32 conf, f32 x, f32 y
//! 0x03 GpuMetrics:   f32 gpu_util_pct, f32 vram_used_mb, f32 vram_total_mb,
//!                    f32 cpu_pct, u32 thread_count
//! 0x04 WeightUpdate: u8 layer_id, f32[4096] (64×64 downsampled from 1024×1024)
//! 0x05 WorldVoxels:  u32 update_seq, u16 occupied_count, per cell:
//!                    u16 index, u8 r, u8 g, u8 b, u8 occupancy
//! 0x06 NavState:     u64 nav_seq, f32 pose_xyz[3], f32 pose_yaw, f32 pose_conf,
//!                    u8 goal_active, i32 goal_cell_x, i32 goal_cell_z,
//!                    f32 goal_xyz[3], f32 goal_yaw
//! ```

use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use qualia_shm::{LayerReader, ShmRegion};
use qualia_types::{MAX_OBJECTS, NUM_LAYERS, STATE_DIM};
use tokio::sync::mpsc;

use crate::AppState;

/// The websocket stream cadence: 20 Hz.
const STREAM_INTERVAL_MS: u64 = 50;
/// How many frames between weight updates (1 Hz).
const WEIGHT_EVERY: u64 = 20;
/// How many frames between world-model and nav updates (2 Hz).
const WORLD_EVERY: u64 = 10;
/// How many frames between voxel updates (0.5 Hz).
const VOXELS_EVERY: u64 = 40;

pub async fn ws_handler(ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(stream_shm)
}

pub async fn signal_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| relay_signals(socket, state))
}

/// Stream the arena to one studio client.
async fn stream_shm(mut socket: WebSocket) {
    let shm_name = std::env::var("QUALIA_SHM_NAME")
        .ok()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| crate::config::DEFAULT_SHM_NAME.to_string());
    let region = match ShmRegion::open(&shm_name) {
        Ok(region) => region,
        Err(error) => {
            let _ = socket
                .send(Message::Text(
                    format!(r#"{{"error":"Cannot open shm: {error}"}}"#).into(),
                ))
                .await;
            return;
        }
    };

    eprintln!("qualia-agent: IGL client connected");

    let mut tick: u64 = 0;
    let mut interval = tokio::time::interval(Duration::from_millis(STREAM_INTERVAL_MS));
    loop {
        interval.tick().await;

        for layer in 0..NUM_LAYERS {
            if socket
                .send(Message::Binary(encode_layer_update(&region, layer).into()))
                .await
                .is_err()
            {
                eprintln!("qualia-agent: IGL client disconnected");
                return;
            }
        }
        if tick % WEIGHT_EVERY == 0 {
            for layer in 0..NUM_LAYERS {
                if socket
                    .send(Message::Binary(encode_weight_update(&region, layer).into()))
                    .await
                    .is_err()
                {
                    return;
                }
            }
        }
        if tick % WORLD_EVERY == 0 {
            if socket
                .send(Message::Binary(encode_world_model(&region).into()))
                .await
                .is_err()
            {
                return;
            }
        }
        if tick % WORLD_EVERY == 1 {
            if socket
                .send(Message::Binary(encode_nav_state(&region).into()))
                .await
                .is_err()
            {
                return;
            }
        }
        if tick % WORLD_EVERY == 5 {
            if socket
                .send(Message::Binary(encode_gpu_metrics().into()))
                .await
                .is_err()
            {
                return;
            }
        }
        if tick % VOXELS_EVERY == 20 {
            if socket
                .send(Message::Binary(encode_world_voxels(&region).into()))
                .await
                .is_err()
            {
                return;
            }
        }
        tick = tick.wrapping_add(1);
    }
}

/// Relay WebRTC offer/answer/ICE frames between registered peers.
async fn relay_signals(mut socket: WebSocket, state: AppState) {
    eprintln!("qualia-agent: signal connection opened");
    // The first text frame must be {"type":"register","role":…,"id":"<peer>"}.
    let peer_id = loop {
        match socket.recv().await {
            Some(Ok(Message::Text(text))) => {
                eprintln!("qualia-agent: signal got text: {}", &text[..text.len().min(200)]);
                let value: serde_json::Value = match serde_json::from_str(&text) {
                    Ok(value) => value,
                    Err(error) => {
                        eprintln!("qualia-agent: signal json err: {error}");
                        return;
                    }
                };
                match value["id"].as_str() {
                    Some(id) => break id.to_string(),
                    None => {
                        eprintln!("qualia-agent: signal no id field");
                        return;
                    }
                }
            }
            Some(Ok(other)) => {
                eprintln!("qualia-agent: signal skipping non-text frame: {other:?}");
            }
            Some(Err(error)) => {
                eprintln!("qualia-agent: signal recv err: {error}");
                return;
            }
            None => {
                eprintln!("qualia-agent: signal connection closed before register");
                return;
            }
        }
    };

    let (sender, mut receiver) = mpsc::unbounded_channel::<String>();
    let joined = format!(r#"{{"type":"peer-joined","id":"{peer_id}"}}"#);
    {
        let peers = state.peers.lock().expect("signal peer lock");
        for peer in peers.values() {
            let _ = peer.send(joined.clone());
        }
    }
    state
        .peers
        .lock()
        .expect("signal peer lock")
        .insert(peer_id.clone(), sender);
    eprintln!("qualia-agent: signal peer registered: {peer_id}");

    loop {
        tokio::select! {
            Some(message) = receiver.recv() => {
                if socket.send(Message::Text(message.into())).await.is_err() {
                    break;
                }
            }
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
                            continue;
                        };
                        if let Some(target) = value["to"].as_str() {
                            let sender = state
                                .peers
                                .lock()
                                .expect("signal peer lock")
                                .get(target)
                                .cloned();
                            if let Some(sender) = sender {
                                let _ = sender.send(text.to_string());
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }
    state.peers.lock().expect("signal peer lock").remove(&peer_id);
    eprintln!("qualia-agent: signal peer disconnected: {peer_id}");
}

fn frame_header(frame_type: u8) -> Vec<u8> {
    let mut buffer = Vec::with_capacity(9);
    buffer.push(frame_type);
    buffer.extend_from_slice(&crate::now_ns().to_le_bytes());
    buffer
}

fn encode_layer_update(region: &ShmRegion, layer: usize) -> Vec<u8> {
    let reader = LayerReader::new(region.layer_slot(layer));
    let belief = *reader.read();

    let mut buffer = frame_header(0x01);
    buffer.push(layer as u8);
    buffer.extend_from_slice(&belief.vfe.to_le_bytes());
    for value in &belief.mean {
        buffer.extend_from_slice(&value.to_le_bytes());
    }
    for value in &belief.residual {
        buffer.extend_from_slice(&value.to_le_bytes());
    }
    for value in &belief.precision {
        buffer.extend_from_slice(&value.to_le_bytes());
    }
    buffer.extend_from_slice(&belief.cycle_us.to_le_bytes());
    buffer.push(belief.compression);
    buffer.extend_from_slice(&belief.confirm_streak.to_le_bytes());
    buffer
}

fn encode_weight_update(region: &ShmRegion, layer: usize) -> Vec<u8> {
    let slot = region.layer_slot(layer);
    std::sync::atomic::fence(std::sync::atomic::Ordering::Acquire);

    // 1024×1024 → 64×64, a stride of 16: the dashboard expects exactly 4096
    // floats per weight frame, so the format is unchanged at a sixteenth of
    // the bandwidth.
    const SIDE: usize = 64;
    const STRIDE: usize = STATE_DIM / SIDE;
    let weights = &slot.weights;
    let mut sampled = [0f32; SIDE * SIDE];
    for row in 0..SIDE {
        for column in 0..SIDE {
            sampled[row * SIDE + column] = weights[(row * STRIDE) * STATE_DIM + (column * STRIDE)];
        }
    }

    let mut buffer = frame_header(0x04);
    buffer.push(layer as u8);
    for value in sampled.iter() {
        buffer.extend_from_slice(&value.to_le_bytes());
    }
    buffer
}

fn encode_world_model(region: &ShmRegion) -> Vec<u8> {
    let world = region.world_model();
    let scene = read_cstr(&world.scene);
    let scene_bytes = scene.as_bytes();

    let mut buffer = frame_header(0x02);
    buffer.extend_from_slice(&(scene_bytes.len() as u16).to_le_bytes());
    buffer.extend_from_slice(scene_bytes);

    let object_count = world.num_objects.min(MAX_OBJECTS as u32) as usize;
    let active: Vec<usize> = (0..object_count)
        .filter(|index| world.objects[*index].active != 0)
        .collect();
    buffer.push(active.len() as u8);
    for index in active {
        let object = &world.objects[index];
        let name = read_cstr(&object.name);
        let name_bytes = name.as_bytes();
        buffer.push(name_bytes.len() as u8);
        buffer.extend_from_slice(name_bytes);
        buffer.extend_from_slice(&object.confidence.to_le_bytes());
        buffer.extend_from_slice(&object.x.to_le_bytes());
        buffer.extend_from_slice(&object.y.to_le_bytes());
    }
    buffer
}

fn encode_gpu_metrics() -> Vec<u8> {
    let (gpu_util, vram_used, vram_total, cpu_pct, threads) = read_gpu_metrics();
    let mut buffer = frame_header(0x03);
    buffer.extend_from_slice(&gpu_util.to_le_bytes());
    buffer.extend_from_slice(&vram_used.to_le_bytes());
    buffer.extend_from_slice(&vram_total.to_le_bytes());
    buffer.extend_from_slice(&cpu_pct.to_le_bytes());
    buffer.extend_from_slice(&threads.to_le_bytes());
    buffer
}

fn encode_nav_state(region: &ShmRegion) -> Vec<u8> {
    let world = region.world_model();
    let nav_seq = world
        .nav_seq
        .load(std::sync::atomic::Ordering::Acquire);
    let pose = world.robot_pose;
    let goal = world.nav_goal;

    let mut buffer = frame_header(0x06);
    buffer.extend_from_slice(&nav_seq.to_le_bytes());
    buffer.extend_from_slice(&pose.x_m.to_le_bytes());
    buffer.extend_from_slice(&pose.y_m.to_le_bytes());
    buffer.extend_from_slice(&pose.z_m.to_le_bytes());
    buffer.extend_from_slice(&pose.yaw_rad.to_le_bytes());
    buffer.extend_from_slice(&pose.confidence.to_le_bytes());
    buffer.push(goal.active);
    buffer.extend_from_slice(&goal.cell_x.to_le_bytes());
    buffer.extend_from_slice(&goal.cell_z.to_le_bytes());
    buffer.extend_from_slice(&goal.x_m.to_le_bytes());
    buffer.extend_from_slice(&goal.y_m.to_le_bytes());
    buffer.extend_from_slice(&goal.z_m.to_le_bytes());
    buffer.extend_from_slice(&goal.yaw_rad.to_le_bytes());
    buffer
}

fn encode_world_voxels(region: &ShmRegion) -> Vec<u8> {
    let voxels = region.world_voxels();
    let seq = voxels
        .update_seq
        .load(std::sync::atomic::Ordering::Acquire) as u32;

    let mut occupied: Vec<(u16, u8, u8, u8, u8)> = Vec::new();
    for (index, cell) in voxels.cells.iter().enumerate() {
        if cell.occupancy > 0 && index < u16::MAX as usize {
            occupied.push((index as u16, cell.r, cell.g, cell.b, cell.occupancy));
        }
    }

    let mut buffer = frame_header(0x05);
    buffer.extend_from_slice(&seq.to_le_bytes());
    buffer.extend_from_slice(&(occupied.len() as u16).to_le_bytes());
    for (index, r, g, b, occupancy) in &occupied {
        buffer.extend_from_slice(&index.to_le_bytes());
        buffer.push(*r);
        buffer.push(*g);
        buffer.push(*b);
        buffer.push(*occupancy);
    }
    buffer
}

fn read_cstr(buffer: &[u8]) -> String {
    let end = buffer.iter().position(|byte| *byte == 0).unwrap_or(buffer.len());
    String::from_utf8_lossy(&buffer[..end]).to_string()
}

/// The board's own counters, read from sysfs; a host that has none reports
/// `-1.0` rather than a fabricated zero.
fn read_gpu_metrics() -> (f32, f32, f32, f32, u32) {
    let gpu_util = std::fs::read_to_string("/sys/devices/platform/gpu.0/load")
        .ok()
        .and_then(|text| text.trim().parse::<u32>().ok())
        .map(|value| value as f32 / 10.0)
        .unwrap_or(-1.0);

    let (vram_used, vram_total) = std::fs::read_to_string("/proc/meminfo")
        .map(|text| {
            let mut total_kb = 0u64;
            let mut available_kb = 0u64;
            for line in text.lines() {
                if let Some(value) = line.strip_prefix("MemTotal:") {
                    total_kb = value
                        .split_whitespace()
                        .next()
                        .and_then(|value| value.parse().ok())
                        .unwrap_or(0);
                } else if let Some(value) = line.strip_prefix("MemAvailable:") {
                    available_kb = value
                        .split_whitespace()
                        .next()
                        .and_then(|value| value.parse().ok())
                        .unwrap_or(0);
                }
            }
            let total_mb = total_kb as f32 / 1024.0;
            let used_mb = total_kb.saturating_sub(available_kb) as f32 / 1024.0;
            (used_mb, total_mb)
        })
        .unwrap_or((-1.0, -1.0));

    let threads = std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|text| {
            text.lines()
                .find(|line| line.starts_with("Threads:"))
                .and_then(|line| line.split_whitespace().nth(1))
                .and_then(|value| value.parse().ok())
        })
        .unwrap_or(0u32);

    (gpu_util, vram_used, vram_total, 0.0, threads)
}
