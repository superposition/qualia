//! The leash's surface, as this stack reads it.
//!
//! On this robot the body's hardware has one owner: `leash serve http` holds the
//! LiDAR's serial port and the drive port and republishes what they carry on its
//! own HTTP surface (D-025). A runner that needs one of those sensors therefore
//! subscribes to the owner, and every subscriber shares this client: one JSON-RPC
//! `tools/call` against `/mcp`, one set of reply shapes.
//!
//! The types here are the parts of `observe` the stack reads. They are parsed
//! leniently on purpose — a missing block is `None`, not an error — because the
//! leash's surface grows and a runner must not fail to start because a sensor it
//! does not use went away.

use serde::Deserialize;
use std::time::Duration;

/// `QUALIA_LEASH_BASE_URL`, default `http://127.0.0.1:8000`.
pub const BASE_URL_ENV: &str = "QUALIA_LEASH_BASE_URL";
/// `QUALIA_SHM_NAME`, default `/qualia_body`.
pub const SHM_NAME_ENV: &str = "QUALIA_SHM_NAME";
/// `QUALIA_LEASH_SENSORS_TIMEOUT_MS`, default `2000`.
pub const TIMEOUT_MS_ENV: &str = "QUALIA_LEASH_SENSORS_TIMEOUT_MS";
/// The leash's HTTP root when none is named.
pub const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8000";
/// The region `qualia-init` creates when the supervisor names no other.
pub const DEFAULT_SHM_NAME: &str = "/qualia_body";
/// Request timeout when none is named.
pub const DEFAULT_TIMEOUT_MS: u64 = 2_000;

/// An HTTP agent with the leash's timeouts applied.
pub fn agent(timeout: Duration) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(timeout)
        .timeout_read(timeout)
        .timeout_write(timeout)
        .build()
}

/// The leash's JSON-RPC reply envelope, as far as the stack reads it.
#[derive(Debug, Deserialize)]
pub struct Reply {
    /// The tool result when the call succeeded; absent on an error reply.
    pub result: Option<ReplyResult>,
}

/// The `result` half of a JSON-RPC reply.
#[derive(Debug, Deserialize)]
pub struct ReplyResult {
    /// The content blocks a tool returned.
    pub content: Vec<Content>,
}

/// One content block of a tool result.
#[derive(Debug, Deserialize)]
pub struct Content {
    /// The block's text, when it carried one.
    pub text: Option<String>,
}

/// One `tools/call` against the leash.
///
/// The body and the reply are carried as strings rather than through `ureq`'s
/// JSON helpers: this workspace's `ureq` is built without its `json` feature
/// (`ureq` 2.12's defaults are `tls` and `gzip` only), and `serde_json` is
/// already the decoder every other JSON boundary here uses. The returned text is
/// the tool's own payload — the caller decodes it into the shape it expects.
pub fn call(agent: &ureq::Agent, base_url: &str, tool: &str) -> Result<String, String> {
    let endpoint = format!("{}/mcp", base_url.trim_end_matches('/'));
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": tool, "arguments": {} },
    });
    let body =
        serde_json::to_string(&request).map_err(|error| format!("encode {tool} request: {error}"))?;
    let response = agent
        .post(&endpoint)
        .set("content-type", "application/json")
        .send_string(&body)
        .map_err(|error| format!("POST {endpoint}: {error}"))?;
    let payload = response
        .into_string()
        .map_err(|error| format!("read {endpoint} reply: {error}"))?;
    let reply: Reply = serde_json::from_str(&payload)
        .map_err(|error| format!("decode {endpoint} reply: {error}"))?;
    reply
        .result
        .and_then(|result| result.content.into_iter().find_map(|content| content.text))
        .ok_or_else(|| format!("{tool} returned no content"))
}

/// Calls the leash's `observe` tool once and returns its sensor set.
pub fn observe(agent: &ureq::Agent, base_url: &str) -> Result<Sensors, String> {
    let text = call(agent, base_url, "observe")?;
    let observe: Observe =
        serde_json::from_str(&text).map_err(|error| format!("decode observe payload: {error}"))?;
    Ok(observe.sensors)
}

/// The `observe` tool's payload, as far as the stack reads it.
#[derive(Debug, Deserialize)]
pub struct Observe {
    /// The sensor set the leash republishes.
    pub sensors: Sensors,
}

/// The sensor set the leash republishes.
#[derive(Debug, Deserialize)]
pub struct Sensors {
    /// The ranging device's block.
    #[serde(default)]
    pub range_scan: Option<RangeScan>,
    /// The inertial block.
    #[serde(default)]
    pub imu: Option<NamedSample>,
    /// The wheel-odometry block.
    #[serde(default)]
    pub odometry: Option<NamedSample>,
    /// The camera block.
    #[serde(default)]
    pub camera: Option<CameraSurface>,
}

impl Sensors {
    /// One line naming every stream the leash reported, so a runner's log
    /// carries the sensor map an operator would otherwise have to ask for.
    pub fn summary(&self) -> String {
        let stream = |name: &str, sample: &Option<NamedSample>| match sample {
            Some(sample) if !sample.status.is_empty() && !sample.source.is_empty() => {
                format!("{name}={}({})", sample.status, sample.source)
            }
            Some(_) => format!("{name}=present"),
            None => format!("{name}=absent"),
        };
        format!(
            "{} {}",
            stream("imu", &self.imu),
            stream("odometry", &self.odometry)
        )
    }
}

/// The camera block's own shape: a surface, not a sample.
#[derive(Debug, Deserialize)]
pub struct CameraSurface {
    /// `healthy`, `degraded`, …
    #[serde(default)]
    pub health: String,
    /// `available`, `unavailable`, …
    #[serde(default)]
    pub status: String,
    /// The still-image path.
    #[serde(default)]
    pub snapshot_url: String,
    /// The MJPEG path `qualia-camera` consumes.
    #[serde(default)]
    pub stream_url: String,
}

/// One sensor block that reports a state and, sometimes, a sample.
#[derive(Debug, Deserialize)]
pub struct NamedSample {
    /// `available`, `unavailable`, …
    #[serde(default)]
    pub status: String,
    /// Who produced it, in the owner's own vocabulary.
    #[serde(default)]
    pub source: String,
}

/// The ranging device's block.
#[derive(Debug, Deserialize)]
pub struct RangeScan {
    /// The owner's error string, when it reported one.
    #[serde(default)]
    pub error: Option<String>,
    /// `available`, `unavailable`, …
    #[serde(default)]
    pub status: String,
    /// The device the owner names, e.g. `waveshare-ugv-ld06`.
    #[serde(default)]
    pub source: String,
    /// The owner's clock for the block, in milliseconds.
    #[serde(default)]
    pub last_ms: u64,
    /// The rotation itself, when one exists.
    #[serde(default)]
    pub sample: Option<ScanSample>,
}

impl RangeScan {
    /// Whether this block carries a rotation a subscriber can publish.
    pub fn is_available(&self) -> bool {
        self.status == "available" && self.sample.is_some()
    }
}

/// One assembled rotation, in the leash's units.
#[derive(Debug, Deserialize)]
pub struct ScanSample {
    /// The first beam's bearing, in radians.
    pub angle_min_rad: f32,
    /// The bearing step between beams, in radians.
    pub angle_increment_rad: f32,
    /// One entry per beam; `None` where the device carried nothing back.
    pub ranges_m: Vec<Option<f32>>,
    /// One intensity per beam, where the device reports them.
    #[serde(default)]
    pub intensities: Vec<Option<f32>>,
    /// The frame the rotation is expressed in.
    #[serde(default)]
    pub frame_id: String,
    /// The owner's measured rotation rate.
    #[serde(default)]
    pub scan_rate_hz: f32,
    /// The owner's clock for this rotation, in milliseconds.
    #[serde(default)]
    pub ts_ms: u64,
}
