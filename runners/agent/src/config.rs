//! Every environment key the process honours, in one place.
//!
//! The keys and their defaults are interface: the stack manifest, the deploy
//! notes and the operators' runbooks name them, so a rewrite keeps them
//! spelled and defaulted exactly as they were.

use std::path::PathBuf;

use qualia_sync_types::ReplicaRole;

use crate::auth::AuthConfig;
use crate::compute::ComputeConfig;
use crate::ThoughtTheaterConfig;

/// `QUALIA_WEB_PORT`, default `8080`.
pub const DEFAULT_WEB_PORT: u16 = 8080;
/// `QUALIA_WEB_DIR`, default `./web/public`.
pub const DEFAULT_WEB_DIR: &str = "./web/public";
/// `QUALIA_JETSON_IP`, default `192.168.0.221` (an extra certificate SAN).
pub const DEFAULT_JETSON_IP: &str = "192.168.0.221";
/// `QUALIA_SHM_NAME`, default `/qualia_body`.
pub const DEFAULT_SHM_NAME: &str = "/qualia_body";
/// `QUALIA_SESSION_STORE`, default `artifacts/qualia_session_store.sqlite`.
pub const DEFAULT_SESSION_STORE: &str = "artifacts/qualia_session_store.sqlite";
/// `QUALIA_MISSION_CONTROL_JOURNAL`, default `artifacts/qualia_mission_control.jsonl`.
pub const DEFAULT_MISSION_JOURNAL: &str = "artifacts/qualia_mission_control.jsonl";
/// `QUALIA_REPLICA_ID`, default `qualia-host`.
pub const DEFAULT_REPLICA_ID: &str = "qualia-host";
/// `QUALIA_ROSBRIDGE_URL`, default `ws://127.0.0.1:9091`.
pub const DEFAULT_ROSBRIDGE_URL: &str = "ws://127.0.0.1:9091";
/// `QUALIA_RERUN_BLUEPRINT_NAME`, default `Thought Theater`.
pub const DEFAULT_BLUEPRINT_NAME: &str = "Thought Theater";
/// `QUALIA_ORIN_CAMERA_DEVICE`, default `/dev/video0`.
pub const DEFAULT_ORIN_CAMERA_DEVICE: &str = "/dev/video0";
/// `QUALIA_ORIN_SNAPSHOT_INTERVAL_MS`, default `500`.
pub const DEFAULT_ORIN_SNAPSHOT_INTERVAL_MS: u64 = 500;

/// Where the mission broker pushes envelopes from, when one is configured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissionBrokerEndpoint {
    /// `QUALIA_MISSION_BROKER_BASE_URL`, trailing slashes trimmed.
    pub base_url: String,
    /// `QUALIA_MISSION_BROKER_TOKEN_FILE`.
    pub token_file: PathBuf,
}

/// Where Leash accepts a forwarded proposal or a stop, when it is configured.
///
/// The base URL is the contract the stack manifest and the runbooks name; the
/// operator token file is only needed to forward a navigation proposal, because
/// Leash's goal route carries the operator's label. A stop needs no token, so an
/// agent configured with the base URL alone can still be stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeashEndpoint {
    /// `QUALIA_LEASH_BASE_URL`, trailing slashes trimmed.
    pub base_url: String,
    /// `QUALIA_LEASH_OPERATOR_TOKEN_FILE`, when the operator supplies one.
    pub operator_token_file: Option<PathBuf>,
}

/// The replica identity this process advertises to its peers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplicaConfig {
    pub id: String,
    pub role: ReplicaRole,
    pub display_name: String,
    pub capabilities_json: String,
    pub metadata_json: String,
    pub endpoint: String,
}

/// The Orin snapshot capture settings, Linux-only in practice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrinConfig {
    pub camera_device: String,
    pub snapshot_interval_ms: u64,
}

/// Everything read from the environment, resolved once at start.
#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub web_port: u16,
    pub web_dir: String,
    pub tls_dir: String,
    pub jetson_ip: String,
    pub shm_name: String,
    pub shm_autocreate: bool,
    pub session_store: String,
    pub mission_journal: String,
    pub compute: ComputeConfig,
    pub thought_theater: ThoughtTheaterConfig,
    pub mission_broker: Option<MissionBrokerEndpoint>,
    pub leash: Option<LeashEndpoint>,
    pub replica: ReplicaConfig,
    pub orin: OrinConfig,
    pub auth: AuthConfig,
    /// `QUALIA_ENTITY_PROFILES`, a path list.
    pub entity_profiles: Option<String>,
    /// `QUALIA_ENTITY_RUNTIME_OVERRIDES_JSON`.
    pub entity_runtime_overrides_json: Option<String>,
    /// `QUALIA_PINKIE_AGENT_URL`, the board's own agent.
    pub pinkie_agent_url: Option<String>,
    pub rerun_viewer_url: String,
    pub rerun_source_url: String,
    pub rerun_blueprint_name: String,
    pub rosbridge_enabled: bool,
    pub rosbridge_url: String,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            web_port: DEFAULT_WEB_PORT,
            web_dir: DEFAULT_WEB_DIR.to_string(),
            tls_dir: default_tls_dir(),
            jetson_ip: DEFAULT_JETSON_IP.to_string(),
            shm_name: DEFAULT_SHM_NAME.to_string(),
            shm_autocreate: false,
            session_store: DEFAULT_SESSION_STORE.to_string(),
            mission_journal: DEFAULT_MISSION_JOURNAL.to_string(),
            compute: ComputeConfig::default(),
            thought_theater: ThoughtTheaterConfig {
                enabled: false,
                viewer_url: String::new(),
                source_url: String::new(),
                blueprint_name: DEFAULT_BLUEPRINT_NAME.to_string(),
            },
            mission_broker: None,
            leash: None,
            replica: ReplicaConfig {
                id: DEFAULT_REPLICA_ID.to_string(),
                role: ReplicaRole::Host,
                display_name: DEFAULT_REPLICA_ID.to_string(),
                capabilities_json: "{}".to_string(),
                metadata_json: "{}".to_string(),
                endpoint: String::new(),
            },
            orin: OrinConfig {
                camera_device: DEFAULT_ORIN_CAMERA_DEVICE.to_string(),
                snapshot_interval_ms: DEFAULT_ORIN_SNAPSHOT_INTERVAL_MS,
            },
            auth: AuthConfig::default(),
            entity_profiles: None,
            entity_runtime_overrides_json: None,
            pinkie_agent_url: None,
            rerun_viewer_url: String::new(),
            rerun_source_url: String::new(),
            rerun_blueprint_name: DEFAULT_BLUEPRINT_NAME.to_string(),
            rosbridge_enabled: true,
            rosbridge_url: DEFAULT_ROSBRIDGE_URL.to_string(),
        }
    }
}

impl AgentConfig {
    /// Resolve the configuration from the process environment.
    pub fn from_env() -> Self {
        let viewer_url = env_string("QUALIA_RERUN_VIEWER_URL").unwrap_or_default();
        let source_url = env_string("QUALIA_RERUN_SOURCE_URL").unwrap_or_default();
        let blueprint_name = env_string("QUALIA_RERUN_BLUEPRINT_NAME")
            .unwrap_or_else(|| DEFAULT_BLUEPRINT_NAME.to_string());
        let replica_id = env_string("QUALIA_REPLICA_ID")
            .unwrap_or_else(|| DEFAULT_REPLICA_ID.to_string());
        let display_name = env_string("QUALIA_REPLICA_DISPLAY_NAME")
            .unwrap_or_else(|| replica_id.clone());
        let mut config = Self {
            web_port: env_string("QUALIA_WEB_PORT")
                .and_then(|value| value.parse().ok())
                .unwrap_or(DEFAULT_WEB_PORT),
            web_dir: env_string("QUALIA_WEB_DIR").unwrap_or_else(|| DEFAULT_WEB_DIR.to_string()),
            tls_dir: env_string("QUALIA_TLS_DIR").unwrap_or_else(default_tls_dir),
            jetson_ip: env_string("QUALIA_JETSON_IP")
                .unwrap_or_else(|| DEFAULT_JETSON_IP.to_string()),
            shm_name: env_string("QUALIA_SHM_NAME")
                .unwrap_or_else(|| DEFAULT_SHM_NAME.to_string()),
            shm_autocreate: env_flag("QUALIA_SHM_AUTOCREATE", false),
            session_store: env_string("QUALIA_SESSION_STORE")
                .unwrap_or_else(|| DEFAULT_SESSION_STORE.to_string()),
            mission_journal: env_string("QUALIA_MISSION_CONTROL_JOURNAL")
                .unwrap_or_else(|| DEFAULT_MISSION_JOURNAL.to_string()),
            compute: ComputeConfig::from_env(),
            thought_theater: ThoughtTheaterConfig {
                enabled: !viewer_url.trim().is_empty(),
                viewer_url,
                source_url,
                blueprint_name,
            },
            mission_broker: mission_broker_from_env(),
            leash: leash_from_env(),
            replica: ReplicaConfig {
                id: replica_id,
                role: env_string("QUALIA_REPLICA_ROLE")
                    .and_then(|value| value.parse::<ReplicaRole>().ok())
                    .unwrap_or(ReplicaRole::Host),
                display_name,
                capabilities_json: env_string("QUALIA_REPLICA_CAPABILITIES_JSON")
                    .unwrap_or_else(|| "{}".to_string()),
                metadata_json: env_string("QUALIA_REPLICA_METADATA_JSON")
                    .unwrap_or_else(|| "{}".to_string()),
                endpoint: env_string("QUALIA_SYNC_ENDPOINT").unwrap_or_default(),
            },
            orin: OrinConfig {
                camera_device: env_string("QUALIA_ORIN_CAMERA_DEVICE")
                    .unwrap_or_else(|| DEFAULT_ORIN_CAMERA_DEVICE.to_string()),
                snapshot_interval_ms: env_string("QUALIA_ORIN_SNAPSHOT_INTERVAL_MS")
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(DEFAULT_ORIN_SNAPSHOT_INTERVAL_MS),
            },
            auth: AuthConfig::from_env(),
            entity_profiles: std::env::var_os("QUALIA_ENTITY_PROFILES")
                .map(|value| value.to_string_lossy().into_owned()),
            entity_runtime_overrides_json: env_string("QUALIA_ENTITY_RUNTIME_OVERRIDES_JSON"),
            pinkie_agent_url: env_string("QUALIA_PINKIE_AGENT_URL"),
            rerun_viewer_url: String::new(),
            rerun_source_url: String::new(),
            rerun_blueprint_name: DEFAULT_BLUEPRINT_NAME.to_string(),
            rosbridge_enabled: env_flag("QUALIA_ROSBRIDGE_ENABLED", true),
            rosbridge_url: env_string("QUALIA_ROSBRIDGE_URL")
                .unwrap_or_else(|| DEFAULT_ROSBRIDGE_URL.to_string()),
        };
        config.rerun_viewer_url = config.thought_theater.viewer_url.clone();
        config.rerun_source_url = config.thought_theater.source_url.clone();
        config.rerun_blueprint_name = config.thought_theater.blueprint_name.clone();
        config
    }
}

/// `$QUALIA_TLS_DIR`, else `$HOME/.qualia_tls`.
fn default_tls_dir() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    format!("{home}/.qualia_tls")
}

/// The certificate the TLS listener presents.
pub fn cert_path(tls_dir: &str) -> String {
    format!("{tls_dir}/cert.pem")
}

/// The private key for [`cert_path`].
pub fn key_path(tls_dir: &str) -> String {
    format!("{tls_dir}/key.pem")
}

fn mission_broker_from_env() -> Option<MissionBrokerEndpoint> {
    let base_url = trimmed_base_url("QUALIA_MISSION_BROKER_BASE_URL")?;
    let token_file = std::env::var_os("QUALIA_MISSION_BROKER_TOKEN_FILE")?;
    Some(MissionBrokerEndpoint {
        base_url,
        token_file: PathBuf::from(token_file),
    })
}

fn leash_from_env() -> Option<LeashEndpoint> {
    let base_url = trimmed_base_url("QUALIA_LEASH_BASE_URL")?;
    let operator_token_file =
        std::env::var_os("QUALIA_LEASH_OPERATOR_TOKEN_FILE").map(PathBuf::from);
    Some(LeashEndpoint {
        base_url,
        operator_token_file,
    })
}

fn trimmed_base_url(name: &str) -> Option<String> {
    env_string(name)
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
}

fn env_string(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.trim().is_empty())
}

/// The stack's boolean spelling: only the reference's five on-spellings are on,
/// so `QUALIA_ROSBRIDGE_ENABLED=Yes` is off exactly as it is in the reference.
pub fn env_flag(name: &str, fallback: bool) -> bool {
    match std::env::var(name) {
        Ok(value) => matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"),
        Err(_) => fallback,
    }
}

/// The auth key's boolean spelling, which is not the stack's: the reference
/// reads `QUALIA_AUTH_ALLOW_LOOPBACK` with an off-only parse, so every value but
/// `0`, `false` and `no` — trimmed, case-insensitively — leaves the loopback
/// bypass on. `Yes`, `on`, `ON`, `True` and `" 1 "` are on here as they are
/// there; a runbook that writes one of them still gets the console its own host
/// is trusted with.
pub fn env_bool(name: &str, fallback: bool) -> bool {
    match std::env::var(name) {
        Ok(value) => !matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "no"
        ),
        Err(_) => fallback,
    }
}

/// Load the entity profiles named by `QUALIA_ENTITY_PROFILES`.
///
/// Failing to read or decode a declared profile is fatal at start: an agent
/// that silently forgets its entities would misreport the fleet.
pub fn load_entity_profiles(config: &AgentConfig) -> Result<Vec<qualia_types::EntityProfile>, String> {
    let Some(paths) = config.entity_profiles.as_deref() else {
        return Ok(Vec::new());
    };
    let paths = std::env::split_paths(paths).collect::<Vec<_>>();
    if paths.is_empty() {
        return Err("QUALIA_ENTITY_PROFILES did not contain any paths".to_string());
    }
    let mut profiles = Vec::new();
    for path in &paths {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("entity profile {} is unreadable: {error}", path.display()))?;
        let profile: qualia_types::EntityProfile = serde_json::from_str(&text)
            .map_err(|error| format!("entity profile {} is invalid: {error}", path.display()))?;
        profiles.push(profile);
    }
    apply_runtime_overrides(
        &mut profiles,
        config.pinkie_agent_url.as_deref(),
        config.entity_runtime_overrides_json.as_deref(),
    );
    Ok(profiles)
}

fn apply_runtime_overrides(
    profiles: &mut [qualia_types::EntityProfile],
    pinkie_agent_url: Option<&str>,
    runtime_overrides_json: Option<&str>,
) {
    let overrides: std::collections::HashMap<String, String> = runtime_overrides_json
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| serde_json::from_str(value).ok())
        .unwrap_or_default();
    for profile in profiles {
        let url = overrides.get(&profile.entity.id).map(String::as_str).or_else(|| {
            pinkie_agent_url.filter(|_| profile.entity.display_name.eq_ignore_ascii_case("pinkie"))
        });
        if let Some(url) = url {
            profile.runtime = Some(qualia_types::EntityRuntime {
                agent_url: url.to_string(),
            });
        }
    }
}
