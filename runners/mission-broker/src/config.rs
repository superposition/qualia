//! Environment-only configuration for the broker.
//!
//! Every address is configuration and every credential is environment. The
//! resolution follows the agent's own `load_secret` discipline
//! (`runners/agent/src/auth.rs`): a value key first, then the matching `_FILE`
//! key, so an operator can hand this process a file path instead of putting a
//! secret into a command line or an environment block that other tooling reads.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// Where the agent's operator surface is, when nothing else is configured.
pub const DEFAULT_AGENT_URL: &str = "http://127.0.0.1:8080";
/// The OpenAI-compatible surface the coach speaks to.
pub const DEFAULT_COACH_BASE_URL: &str = "https://api.deepseek.com";
/// The coach's model.
pub const DEFAULT_COACH_MODEL: &str = "deepseek-chat";
/// A decision that has not arrived by then degrades to no decision.
pub const DEFAULT_COACH_TIMEOUT_MS: u64 = 8_000;
/// The console's Coach panel default, and the only default this crate binds.
pub const DEFAULT_STATUS_PORT: u16 = 8091;
/// The status surface is loopback or it is not served.
pub const DEFAULT_STATUS_HOST: &str = "127.0.0.1";
/// Gap between two ticks of a multi-tick run.
pub const DEFAULT_TICK_MS: u64 = 1_000;

/// The bounded operating area a mission envelope carries, default `±3 m` odom.
pub const DEFAULT_AREA_M: f32 = 3.0;
/// Defaults for the numeric envelope: the agent's low-speed limits, and the
/// broker's, not the model's — a model chooses the decision and the objective,
/// never the robot's ceiling.
pub const DEFAULT_SPEED_CEILING_MPS: f32 = 0.15;
pub const DEFAULT_MAX_DISTANCE_M: f32 = 2.0;
pub const DEFAULT_MAX_RUNTIME_MS: u64 = 60_000;
pub const DEFAULT_MAX_REPLANS: u8 = 1;
pub const DEFAULT_EVIDENCE_MAX_AGE_MS: u64 = 1_500;

/// The coach's credential, in resolution order.
pub const COACH_KEY_KEYS: [&str; 2] = ["DEEPSEEK_API_KEY", "QUALIA_COACH_API_KEY"];
/// The coach's credential as a file path, in resolution order.
pub const COACH_KEY_FILE_KEYS: [&str; 2] = ["DEEPSEEK_API_KEY_FILE", "QUALIA_COACH_API_KEY_FILE"];

/// The bounded rectangle a mission may operate in, odom metres.
#[derive(Debug, Clone, PartialEq)]
pub struct Area {
    pub min_x_m: f32,
    pub min_y_m: f32,
    pub max_x_m: f32,
    pub max_y_m: f32,
}

impl Default for Area {
    fn default() -> Self {
        Self {
            min_x_m: -DEFAULT_AREA_M,
            min_y_m: -DEFAULT_AREA_M,
            max_x_m: DEFAULT_AREA_M,
            max_y_m: DEFAULT_AREA_M,
        }
    }
}

impl Area {
    /// The area as the mission envelope's operating-area member.
    pub fn envelope(&self) -> qualia_sync_types::MissionOperatingAreaV1 {
        qualia_sync_types::MissionOperatingAreaV1 {
            frame_id: "odom".to_string(),
            min_x_m: self.min_x_m,
            min_y_m: self.min_y_m,
            max_x_m: self.max_x_m,
            max_y_m: self.max_y_m,
        }
    }
}

/// The coach's configuration, resolved from the environment.
#[derive(Clone)]
pub struct CoachConfig {
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    /// The environment key the credential came from, for the named line.
    pub key_source: Option<String>,
    pub timeout: Duration,
}

impl CoachConfig {
    /// Resolve the coach from the environment.
    pub fn from_env() -> Self {
        let (api_key, key_source) = load_secret(&COACH_KEY_KEYS, &COACH_KEY_FILE_KEYS);
        Self {
            base_url: env_string("QUALIA_COACH_BASE_URL")
                .map(|value| trim_slashes(&value))
                .unwrap_or_else(|| DEFAULT_COACH_BASE_URL.to_string()),
            model: env_string("QUALIA_COACH_MODEL").unwrap_or_else(|| DEFAULT_COACH_MODEL.to_string()),
            api_key,
            key_source,
            timeout: Duration::from_millis(
                env_u64("QUALIA_COACH_TIMEOUT_MS").unwrap_or(DEFAULT_COACH_TIMEOUT_MS),
            ),
        }
    }

    /// Whether a credential is present, without exposing it.
    pub fn configured(&self) -> bool {
        self.api_key.is_some()
    }
}

/// Deliberately not derived: a `{:?}` on the configuration must not be a way
/// to reach the credential, so this prints what [`crate::redact::key_presence`]
/// prints — presence and provenance — and never `api_key`.
impl std::fmt::Debug for CoachConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CoachConfig")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field(
                "key_presence",
                &self
                    .key_source
                    .as_deref()
                    .map(|source| crate::redact::key_presence(Some(source))),
            )
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// The broker's whole configuration.
#[derive(Debug, Clone)]
pub struct BrokerConfig {
    pub agent_url: String,
    pub agent_tls_dir: Option<PathBuf>,
    pub broker_token: Option<String>,
    pub broker_id: String,
    /// Files carrying the braid's proposal envelopes, if any were named.
    pub proposals: Vec<PathBuf>,
    pub status_host: String,
    pub status_port: u16,
    pub status_enabled: bool,
    pub tick: Duration,
    pub fly_governed: bool,
    pub area: Area,
    pub coach: CoachConfig,
}

impl BrokerConfig {
    /// Resolve every value from the environment.
    pub fn from_env() -> Self {
        let (broker_token, _) = load_secret(
            &["QUALIA_MISSION_BROKER_TOKEN"],
            &["QUALIA_MISSION_BROKER_TOKEN_FILE"],
        );
        let status_host = env_string("QUALIA_COACH_STATUS_HOST")
            .unwrap_or_else(|| DEFAULT_STATUS_HOST.to_string());
        let status_port = env_u64("QUALIA_COACH_STATUS_PORT")
            .and_then(|value| u16::try_from(value).ok())
            .unwrap_or(DEFAULT_STATUS_PORT);
        Self {
            agent_url: env_string("QUALIA_AGENT_URL")
                .map(|value| trim_slashes(&value))
                .unwrap_or_else(|| DEFAULT_AGENT_URL.to_string()),
            agent_tls_dir: env_string("QUALIA_AGENT_TLS_DIR")
                .or_else(|| env_string("QUALIA_TLS_DIR"))
                .map(PathBuf::from),
            broker_token,
            broker_id: env_string("QUALIA_MISSION_BROKER_ID")
                .unwrap_or_else(|| format!("qualia-mission-broker-{}", host_slug())),
            proposals: env_string("QUALIA_MISSION_BROKER_PROPOSALS")
                .map(|value| vec![PathBuf::from(value)])
                .unwrap_or_default(),
            status_host,
            status_port,
            status_enabled: true,
            tick: Duration::from_millis(env_u64("QUALIA_COACH_TICK_MS").unwrap_or(DEFAULT_TICK_MS)),
            fly_governed: env_bool("QUALIA_MISSION_BROKER_FLY_GOVERNED", false),
            area: Area::default(),
            coach: CoachConfig::from_env(),
        }
    }
}

/// A host name reduced to the identifier alphabet the mission envelope allows.
pub fn host_slug() -> String {
    let raw = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "host".to_string());
    let mut slug = String::with_capacity(raw.len());
    for character in raw.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "host".to_string()
    } else {
        slug.chars().take(48).collect()
    }
}

/// Read a secret from the first value key that names one, else the first file
/// key, returning the key it came from as its provenance.
pub fn load_secret(value_keys: &[&str], file_keys: &[&str]) -> (Option<String>, Option<String>) {
    for key in value_keys {
        if let Some(value) = env_string(key) {
            return (Some(value), Some((*key).to_string()));
        }
    }
    for key in file_keys {
        let Some(path) = std::env::var_os(key) else {
            continue;
        };
        if let Some(value) = std::fs::read_to_string(Path::new(&path))
            .ok()
            .and_then(non_empty)
        {
            return (Some(value), Some((*key).to_string()));
        }
    }
    (None, None)
}

fn non_empty(value: String) -> Option<String> {
    let trimmed = value.trim().to_string();
    (!trimmed.is_empty()).then_some(trimmed)
}

/// A trimmed, non-empty environment string.
pub fn env_string(key: &str) -> Option<String> {
    std::env::var(key).ok().and_then(non_empty)
}

/// An environment integer, when the key names one.
pub fn env_u64(key: &str) -> Option<u64> {
    env_string(key).and_then(|value| value.parse().ok())
}

/// An environment boolean: `1`/`true`/`yes`/`on`, case-insensitive.
pub fn env_bool(key: &str, default: bool) -> bool {
    match env_string(key) {
        Some(value) => matches!(
            value.to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        None => default,
    }
}

fn trim_slashes(value: &str) -> String {
    value.trim_end_matches('/').to_string()
}
