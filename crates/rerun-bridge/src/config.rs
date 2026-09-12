//! Bridge configuration and the error type the bridge hands back.

use std::fmt::{Display, Formatter};
use std::path::PathBuf;

/// Where the recording goes.
///
/// `Buffered` keeps the recording in memory (the viewer connects to it),
/// `Save` streams it to an `.rrd` file, `Connect` streams it to a running
/// viewer or server over gRPC (the live path: the viewer is on the desktop and
/// the logger is on the board), and `Disabled` makes every bridge call a no-op
/// so a caller can leave instrumentation in place unconditionally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RerunSinkConfig {
    Disabled,
    Buffered,
    Save(PathBuf),
    /// A Rerun proxy URL, e.g. `rerun+http://127.0.0.1:9876/proxy`.
    Connect(String),
}

/// Application id plus destination for one bridge instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RerunBridgeConfig {
    pub application_id: String,
    pub sink: RerunSinkConfig,
}

impl Default for RerunBridgeConfig {
    fn default() -> Self {
        Self {
            application_id: "qualia-rerun-bridge".to_string(),
            sink: RerunSinkConfig::Buffered,
        }
    }
}

/// A bridge failure, with the operation that failed folded into the message.
#[derive(Debug)]
pub struct BridgeError {
    detail: String,
}

impl BridgeError {
    pub(crate) fn new(context: &str, error: impl Display) -> Self {
        Self {
            detail: format!("{context}: {error}"),
        }
    }
}

impl Display for BridgeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.detail)
    }
}

impl std::error::Error for BridgeError {}
