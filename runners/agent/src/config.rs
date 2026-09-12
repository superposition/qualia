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
/// `QUALIA_JEPA_CATALOG`, default `artifacts/jepa/catalog.json`, the catalog of
/// sealed session evidence Step 31's dataset binary reads.
pub const DEFAULT_JEPA_CATALOG: &str = "artifacts/jepa/catalog.json";
/// `QUALIA_JEPA_DATASET_DIR`, default `artifacts/jepa/datasets` — the dataset
/// binary's own default output directory.
pub const DEFAULT_JEPA_DATASET_DIR: &str = "artifacts/jepa/datasets";
/// `QUALIA_JEPA_CHECKPOINT_DIR`, default `artifacts/jepa/checkpoints` — the
/// trainer's own default output directory.
pub const DEFAULT_JEPA_CHECKPOINT_DIR: &str = "artifacts/jepa/checkpoints";
/// `QUALIA_JEPA_BACKEND`, default `cpu`; the JEPA runtime reads the same key.
pub const DEFAULT_JEPA_BACKEND: &str = "cpu";
/// `QUALIA_JEPA_REGISTRY`, default `artifacts/jepa/registry.turso` — the
/// registry command line's own default database.
pub const DEFAULT_JEPA_REGISTRY: &str = "artifacts/jepa/registry.turso";
/// `QUALIA_JEPA_GENERATION_FILE`, default `artifacts/jepa/active-generation.json`
/// — the registry command line's own default pointer file, and the same key the
/// JEPA runtime hot-swaps.
pub const DEFAULT_JEPA_GENERATION_FILE: &str = "artifacts/jepa/active-generation.json";

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

/// Step 31's paths, as the process's environment names them: the sealed catalog
/// the dataset binary reads, where the manifest and the candidate are written,
/// the trainer backend, the registry the gates run against, and the generation
/// pointer a promotion moves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromotionConfig {
    /// `QUALIA_JEPA_CATALOG`.
    pub catalog: PathBuf,
    /// `QUALIA_JEPA_DATASET_DIR`, where the immutable manifest lands.
    pub dataset_dir: PathBuf,
    /// `QUALIA_JEPA_CHECKPOINT_DIR`, where the trainer writes the candidate.
    pub checkpoint_dir: PathBuf,
    /// `QUALIA_JEPA_BACKEND`; the JEPA runtime resolves the same key.
    pub backend: String,
    /// `QUALIA_JEPA_REGISTRY`, the gates' database.
    pub registry: PathBuf,
    /// `QUALIA_JEPA_GENERATION_FILE`, the pointer the registry publishes and
    /// the JEPA runtime hot-swaps between ticks.
    pub generation_file: PathBuf,
}

impl Default for PromotionConfig {
    fn default() -> Self {
        Self {
            catalog: PathBuf::from(DEFAULT_JEPA_CATALOG),
            dataset_dir: PathBuf::from(DEFAULT_JEPA_DATASET_DIR),
            checkpoint_dir: PathBuf::from(DEFAULT_JEPA_CHECKPOINT_DIR),
            backend: DEFAULT_JEPA_BACKEND.to_string(),
            registry: PathBuf::from(DEFAULT_JEPA_REGISTRY),
            generation_file: PathBuf::from(DEFAULT_JEPA_GENERATION_FILE),
        }
    }
}

impl PromotionConfig {
    /// Resolve the paths from the process environment.
    pub fn from_env() -> Self {
        Self {
            catalog: env_path("QUALIA_JEPA_CATALOG", DEFAULT_JEPA_CATALOG),
            dataset_dir: env_path("QUALIA_JEPA_DATASET_DIR", DEFAULT_JEPA_DATASET_DIR),
            checkpoint_dir: env_path("QUALIA_JEPA_CHECKPOINT_DIR", DEFAULT_JEPA_CHECKPOINT_DIR),
            backend: env_string("QUALIA_JEPA_BACKEND")
                .unwrap_or_else(|| DEFAULT_JEPA_BACKEND.to_string()),
            registry: env_path("QUALIA_JEPA_REGISTRY", DEFAULT_JEPA_REGISTRY),
            generation_file: env_path("QUALIA_JEPA_GENERATION_FILE", DEFAULT_JEPA_GENERATION_FILE),
        }
    }

    /// The improvement loop's view of the same paths.
    pub fn improvement(&self) -> qualia_braid::improvement::ImprovementConfig {
        qualia_braid::improvement::ImprovementConfig {
            catalog: self.catalog.clone(),
            dataset_dir: self.dataset_dir.clone(),
            checkpoint_dir: self.checkpoint_dir.clone(),
            backend: self.backend.clone(),
        }
    }
}

/// An environment key holding a path, or its default.
fn env_path(name: &str, fallback: &str) -> PathBuf {
    env_string(name)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(fallback))
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
    /// `QUALIA_STACK_MANIFEST`: the manifest the supervisor spawns the stack's
    /// children from, when the environment names one. There is no default: a
    /// launch that names none has no manifest the supervisor will read the
    /// coupling dial from (`runners/init` falls back to its *embedded*
    /// `config/stack-manifest.default.json`), so the handover is inert rather
    /// than rewriting the product's tracked manifest in place (T30, #46).
    pub stack_manifest: Option<String>,
    pub compute: ComputeConfig,
    pub thought_theater: ThoughtTheaterConfig,
    pub mission_broker: Option<MissionBrokerEndpoint>,
    pub leash: Option<LeashEndpoint>,
    /// Step 31's paths: the improvement loop's inputs and the promotion's
    /// registry pointer.
    pub promotion: PromotionConfig,
    pub replica: ReplicaConfig,
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
            stack_manifest: None,
            compute: ComputeConfig::default(),
            thought_theater: ThoughtTheaterConfig {
                enabled: false,
                viewer_url: String::new(),
                source_url: String::new(),
                blueprint_name: DEFAULT_BLUEPRINT_NAME.to_string(),
            },
            mission_broker: None,
            leash: None,
            promotion: PromotionConfig::default(),
            replica: ReplicaConfig {
                id: DEFAULT_REPLICA_ID.to_string(),
                role: ReplicaRole::Host,
                display_name: DEFAULT_REPLICA_ID.to_string(),
                capabilities_json: "{}".to_string(),
                metadata_json: "{}".to_string(),
                endpoint: String::new(),
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
            stack_manifest: env_string("QUALIA_STACK_MANIFEST"),
            compute: ComputeConfig::from_env(),
            thought_theater: ThoughtTheaterConfig {
                enabled: !viewer_url.trim().is_empty(),
                viewer_url,
                source_url,
                blueprint_name,
            },
            mission_broker: mission_broker_from_env(),
            leash: leash_from_env(),
            promotion: PromotionConfig::from_env(),
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
