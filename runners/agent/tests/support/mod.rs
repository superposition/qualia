//! Shared harness for the agent's HTTP surface tests.
//!
//! Each integration test binary compiles this module whole, so a helper only one
//! of them reaches for is still dead code in the other; the harness is one file
//! on purpose.
#![allow(dead_code)]
//!
//! The router is exercised in-process through `tower`'s `oneshot`, with the
//! `ConnectInfo` extension a real socket would carry inserted by hand, so a
//! request reaches exactly the same handler code path as one arriving over TLS.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::connect_info::ConnectInfo;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use qualia_agent::auth::AuthConfig;
use qualia_agent::config::AgentConfig;
use qualia_agent::{app, AppState};
use tower::ServiceExt;

pub const BROKER_TOKEN: &str = "broker-test-token";
pub const ADMIN_TOKEN: &str = "admin-test-token";

pub struct Harness {
    pub router: Router,
    pub state: AppState,
    /// The scratch stack manifest the coupling dial is written to, so a test
    /// never rewrites the repository's own `config/stack-manifest.default.json`.
    pub stack_manifest: PathBuf,
    /// Kept alive so the scratch files outlive the router.
    pub _dir: tempfile::TempDir,
}

impl Harness {
    /// A state wired to scratch storage: its own SHM region name, its own
    /// journal file and its own web directory, so no test can see another's
    /// bytes.
    pub fn new() -> Self {
        Self::with(|_| {})
    }

    /// The same scratch state, with `configure` applied to the resolved config
    /// before the state is built — for the tests that need a different auth
    /// posture or a different subsystem under test.
    pub fn with(configure: impl FnOnce(&mut AgentConfig)) -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut config = AgentConfig::default();
        config.web_dir = dir.path().join("web").to_string_lossy().into_owned();
        config.tls_dir = dir.path().join("tls").to_string_lossy().into_owned();
        config.shm_name = scratch_shm_name();
        config.shm_autocreate = true;
        config.mission_journal = dir
            .path()
            .join("mission-control.jsonl")
            .to_string_lossy()
            .into_owned();
        let stack_manifest = dir.path().join("stack-manifest.json");
        config.stack_manifest = Some(stack_manifest.to_string_lossy().into_owned());
        std::fs::write(
            &stack_manifest,
            r#"{"schema_version":"qualia.stack.v1","env":{"QUALIA_FLY_MODE":"off"}}"#,
        )
        .expect("stack manifest");
        config.auth = AuthConfig {
            read_token: None,
            peer_token: None,
            compute_token: None,
            mission_broker_token: Some(Arc::from(BROKER_TOKEN)),
            admin_token: Some(Arc::from(ADMIN_TOKEN)),
            allow_loopback: true,
        };
        std::fs::create_dir_all(&config.web_dir).expect("web dir");
        std::fs::write(
            dir.path().join("web").join("index.html"),
            "<!doctype html><title>operator</title>",
        )
        .expect("index");
        configure(&mut config);
        let state = qualia_agent::build_state(config).expect("agent state");
        Self {
            router: app(state.clone()),
            state,
            stack_manifest,
            _dir: dir,
        }
    }

    pub async fn get(&self, uri: &str) -> Reply {
        self.send(request("GET", uri, None, None)).await
    }

    /// `GET` with a bearer token, for the auth postures a test configures.
    pub async fn get_with_token(&self, uri: &str, token: &str) -> Reply {
        self.send(request("GET", uri, Some(token), None)).await
    }

    pub async fn post_json(&self, uri: &str, token: Option<&str>, body: serde_json::Value) -> Reply {
        self.send(request("POST", uri, token, Some(body))).await
    }

    async fn send(&self, request: Request<Body>) -> Reply {
        let response = self
            .router
            .clone()
            .oneshot(request)
            .await
            .expect("router answers");
        let status = response.status();
        let content_type = response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes()
            .to_vec();
        Reply {
            status,
            content_type,
            bytes,
        }
    }
}

pub struct Reply {
    pub status: StatusCode,
    pub content_type: Option<String>,
    pub bytes: Vec<u8>,
}

impl Reply {
    /// The response's `Content-Type`, when it set one.
    pub fn content_type(&self) -> Option<&str> {
        self.content_type.as_deref()
    }
}

impl Reply {
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.bytes).unwrap_or_else(|error| {
            panic!(
                "body is not JSON ({error}): {}",
                String::from_utf8_lossy(&self.bytes)
            )
        })
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }
}

fn request(
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<serde_json::Value>,
) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let body = match body {
        Some(value) => {
            builder = builder.header(axum::http::header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(&value).expect("request encodes"))
        }
        None => Body::empty(),
    };
    builder
        .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 41_000))))
        .body(body)
        .expect("request builds")
}

fn scratch_shm_name() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let serial = NEXT.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos() as u64)
        .unwrap_or(0);
    format!("/qualia-agent-test-{}-{nanos}-{serial}", std::process::id())
}

/// A mission envelope that satisfies every bound `qualia-sync-types` checks.
pub fn envelope(
    mission_id: &str,
    idempotency_key: &str,
    sequence: u64,
    command: &str,
) -> serde_json::Value {
    let issued_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(1_700_000_000_000);
    serde_json::json!({
        "schema_version": "qualia.mission-envelope.v1",
        "broker_id": "broker-test",
        "producer_epoch": 1,
        "sequence": sequence,
        "mission_id": mission_id,
        "idempotency_key": idempotency_key,
        "command": command,
        "issued_at_ms": issued_at_ms,
        "deadline_ms": issued_at_ms + 60_000,
        "objective": {
            "kind": "explore_frontier",
            "summary": "look at the far corner of the arena",
            "target_x_m": null,
            "target_y_m": null,
            "tolerance_m": null
        },
        "constraints": {
            "operating_area": {
                "frame_id": "odom",
                "min_x_m": -3.0,
                "min_y_m": -3.0,
                "max_x_m": 3.0,
                "max_y_m": 3.0
            },
            "speed_ceiling_mps": 0.2,
            "max_distance_m": 2.0,
            "max_runtime_ms": 60_000,
            "max_replans": 2,
            "evidence_max_age_ms": 1_000
        },
        "evidence_refs": ["evidence-1"],
        "fly_governed": false
    })
}
