//! Projection-only bridge from Qualia state into a Rerun recording.
//!
//! The bridge never mutates canonical, sync, or control state. It reads typed
//! world-model and sync slices and writes them at stable entity paths so a
//! viewer can follow proposals, accepted canonical state, the operational hot
//! path, coach decisions, and replication health.
//!
//! ```no_run
//! use qualia_rerun_bridge::{QualiaRerunBridge, RerunBridgeConfig, RerunSinkConfig};
//!
//! let bridge = QualiaRerunBridge::new(&RerunBridgeConfig {
//!     application_id: "qualia".to_string(),
//!     sink: RerunSinkConfig::Buffered,
//! })?;
//! bridge.send_default_thought_theater_blueprint()?;
//! # Ok::<(), qualia_rerun_bridge::BridgeError>(())
//! ```

mod blueprint;
mod bridge;
mod config;
mod connectome;
mod records;
mod render;
mod taxonomy;

pub use blueprint::{
    default_connectome_blueprint, default_graph_blueprint, default_thought_theater_blueprint,
};
pub use bridge::QualiaRerunBridge;
pub use config::{BridgeError, RerunBridgeConfig, RerunSinkConfig};
pub use connectome::{
    cell_type_color, collect_connectome_cloud_records, collect_connectome_tick_records,
    firing_color, ConnectomeCloud, ConnectomeEntityTaxonomy, ConnectomeNeuron,
    ConnectomeProjection, ConnectomeProjectionContent, ConnectomeProjectionRecord,
    CLOUD_POINT_RADIUS, FIRING_POINT_RADIUS,
};
pub use records::{
    collect_sync_projection_records, collect_world_model_projection_records, SessionReplayFrame,
    SyncProjection, WorldModelProjection, WorldModelProjectionContent, WorldModelProjectionRecord,
};
pub use taxonomy::{ReplayEntityTaxonomy, SyncEntityTaxonomy, WorldModelEntityTaxonomy};
