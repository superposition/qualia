//! Bearer-token scopes for the operator surface.
//!
//! Loopback callers are trusted for read-only work — that is what lets the
//! console on the same machine poll without a secret — but never for the two
//! scopes that change what the robot does: authorization and mission intake.

use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;

use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;

use crate::config::env_flag;

/// What a caller is asking to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthScope {
    /// Inspect state.
    Read,
    /// Exchange sync payloads with a peer replica.
    Peer,
    /// Submit planner or compute work.
    Compute,
    /// Deliver a mission envelope.
    MissionBroker,
    /// Authorize a mission's motion, or drain the broker.
    Admin,
}

/// The tokens this process accepts, resolved once at start.
#[derive(Debug, Clone)]
pub struct AuthConfig {
    pub read_token: Option<Arc<str>>,
    pub peer_token: Option<Arc<str>>,
    pub compute_token: Option<Arc<str>>,
    pub mission_broker_token: Option<Arc<str>>,
    pub admin_token: Option<Arc<str>>,
    /// Whether a loopback caller skips the Read/Peer/Compute scopes.
    pub allow_loopback: bool,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            read_token: None,
            peer_token: None,
            compute_token: None,
            mission_broker_token: None,
            admin_token: None,
            allow_loopback: true,
        }
    }
}

impl AuthConfig {
    /// Read each scope's token from its value key, else its file key.
    pub fn from_env() -> Self {
        Self {
            read_token: load_secret("QUALIA_READ_TOKEN", "QUALIA_READ_TOKEN_FILE"),
            peer_token: load_secret("QUALIA_SYNC_TOKEN", "QUALIA_SYNC_TOKEN_FILE"),
            compute_token: load_secret("QUALIA_COMPUTE_TOKEN", "QUALIA_COMPUTE_TOKEN_FILE"),
            mission_broker_token: load_secret(
                "QUALIA_MISSION_BROKER_TOKEN",
                "QUALIA_MISSION_BROKER_TOKEN_FILE",
            ),
            admin_token: load_secret("QUALIA_ADMIN_TOKEN", "QUALIA_ADMIN_TOKEN_FILE"),
            allow_loopback: env_flag("QUALIA_AUTH_ALLOW_LOOPBACK", true),
        }
    }

    /// Admit `scope` for this request, or describe the refusal.
    pub fn authorize(
        &self,
        scope: AuthScope,
        headers: &HeaderMap,
        remote_ip: IpAddr,
    ) -> Result<(), Response> {
        let loopback_bypass = remote_ip.is_loopback()
            && self.allow_loopback
            && !matches!(scope, AuthScope::Admin | AuthScope::MissionBroker);
        if loopback_bypass {
            return Ok(());
        }
        let presented =
            bearer_token(headers).ok_or_else(|| unauthorized())?;
        let accepted = match scope {
            AuthScope::Read => [self.read_token.as_deref(), self.admin_token.as_deref()],
            AuthScope::Peer => [self.peer_token.as_deref(), self.admin_token.as_deref()],
            AuthScope::Compute => [self.compute_token.as_deref(), self.admin_token.as_deref()],
            AuthScope::MissionBroker => [
                self.mission_broker_token.as_deref(),
                self.admin_token.as_deref(),
            ],
            AuthScope::Admin => [self.admin_token.as_deref(), None],
        };
        if accepted
            .into_iter()
            .flatten()
            .any(|expected| constant_time_eq(expected.as_bytes(), presented.as_bytes()))
        {
            Ok(())
        } else {
            Err(unauthorized())
        }
    }

    /// The token a peer replica authenticates with.
    pub fn peer_token(&self) -> Option<&str> {
        self.peer_token.as_deref()
    }
}

fn load_secret(value_key: &str, file_key: &str) -> Option<Arc<str>> {
    std::env::var(value_key)
        .ok()
        .and_then(non_empty)
        .or_else(|| {
            let path = std::env::var_os(file_key)?;
            std::fs::read_to_string(Path::new(&path))
                .ok()
                .and_then(non_empty)
        })
        .map(Arc::<str>::from)
}

fn non_empty(value: String) -> Option<String> {
    let trimmed = value.trim().to_string();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let token = value.strip_prefix("Bearer ")?;
    (!token.is_empty()).then_some(token)
}

/// Compare without leaking where the first difference is.
fn constant_time_eq(expected: &[u8], presented: &[u8]) -> bool {
    if expected.len() != presented.len() {
        return false;
    }
    let mut difference = 0u8;
    for (left, right) in expected.iter().zip(presented) {
        difference |= left ^ right;
    }
    difference == 0
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({ "error": "a valid bearer token is required" })),
    )
        .into_response()
}
