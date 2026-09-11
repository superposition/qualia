//! `qualia-session-store`: the durable SQLite record of sessions, abstraction
//! epochs, missions, planning runs and world-model curation.
//!
//! The store owns one connection per process. Everything that other crates and
//! the `qualia-session` CLI read — table names, column names, indices and the
//! `user_version` marker — is a contract; the row and upsert types below are the
//! typed view onto it.

pub mod mission_types;
mod rosbag2;
mod rows;
mod store_analysis;
mod store_missions;
mod store_sessions;
mod sync;
mod world_model;
pub use rosbag2::*;
pub use sync::*;

use crate::mission_types::{
    AnalysisJobKind, AnalysisJobStatus, CandidateState, DomainKind, ExactnessKind, GraphForm,
    GraphKind, LinkKind, MessageDirection, PlanRunRow, PlanStatus, RegionKind, WorldRegionRow,
};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

/// The schema revision this build writes and accepts. A database stamped with a
/// higher revision was written by a newer build and is refused rather than
/// guessed at.
const SCHEMA_VERSION: i64 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionRow {
    pub id: i64,
    pub path: String,
    pub filename: String,
    pub media_kind: String,
    pub analysis_kind: String,
    pub status: String,
    pub duration_sec: f64,
    pub imported_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionUpsert {
    pub path: String,
    pub filename: String,
    pub media_kind: String,
    pub analysis_kind: String,
    pub status: String,
    pub duration_sec: f64,
    pub imported_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionStreamRow {
    pub id: i64,
    pub session_id: i64,
    pub stream_key: String,
    pub stream_kind: String,
    pub role: String,
    pub path: String,
    pub sync_group: String,
    pub metadata_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionStreamUpsert {
    pub session_id: i64,
    pub stream_key: String,
    pub stream_kind: String,
    pub role: String,
    pub path: String,
    pub sync_group: String,
    pub metadata_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AbstractionEpochRow {
    pub id: i64,
    pub session_id: i64,
    pub epoch_index: i64,
    pub layer_name: String,
    pub abstraction_family: String,
    pub status: String,
    pub created_at: String,
    pub completed_at: Option<String>,
    pub summary_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AbstractionEpochUpsert {
    pub session_id: i64,
    pub epoch_index: i64,
    pub layer_name: String,
    pub abstraction_family: String,
    pub status: String,
    pub created_at: String,
    pub completed_at: Option<String>,
    pub summary_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AbstractionSpaceRow {
    pub id: i64,
    pub epoch_id: i64,
    pub space_family: String,
    pub abstraction_name: String,
    pub source_kind: String,
    pub representation_kind: String,
    pub uncertainty_kind: String,
    pub dimensionality: i64,
    pub schema_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AbstractionSpaceUpsert {
    pub epoch_id: i64,
    pub space_family: String,
    pub abstraction_name: String,
    pub source_kind: String,
    pub representation_kind: String,
    pub uncertainty_kind: String,
    pub dimensionality: i64,
    pub schema_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AbstractStateSample {
    pub step: i64,
    pub timestamp_sec: f64,
    pub symbol_key: String,
    pub payload_json: String,
    pub confidence: f64,
    pub sample_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AbstractStateSampleRow {
    pub epoch_id: i64,
    pub space_id: i64,
    pub step: i64,
    pub timestamp_sec: f64,
    pub symbol_key: String,
    pub payload_json: String,
    pub confidence: f64,
    pub sample_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AnalysisJobUpsert {
    pub session_id: i64,
    pub environment_id: Option<i64>,
    pub job_kind: AnalysisJobKind,
    pub status: AnalysisJobStatus,
    pub requested_at: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub window_start_sec: Option<f64>,
    pub window_end_sec: Option<f64>,
    pub spec_json: String,
    pub summary_json: String,
    pub failure_json: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GraphFragmentUpsert {
    pub analysis_job_id: i64,
    pub session_id: i64,
    pub epoch_id: Option<i64>,
    pub stream_key: String,
    pub fragment_key: String,
    pub graph_kind: GraphKind,
    pub graph_form: GraphForm,
    pub exactness: ExactnessKind,
    pub variable_count: i64,
    pub factor_count: i64,
    pub tree_width: Option<i64>,
    pub root_variable_key: Option<String>,
    pub window_start_sec: f64,
    pub window_end_sec: f64,
    pub summary_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BeliefMatrixUpsert {
    pub fragment_id: i64,
    pub variable_key: String,
    pub domain_kind: DomainKind,
    pub normalization_error: f64,
    pub entropy: f64,
    pub max_state_key: String,
    pub values_json: String,
    pub matrix_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MessageSnapshotUpsert {
    pub fragment_id: i64,
    pub edge_key: String,
    pub direction: MessageDirection,
    pub iteration_index: i64,
    pub source_node_key: String,
    pub target_node_key: String,
    pub values_json: String,
    pub residual_norm: f64,
    pub message_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorldRegionUpsert {
    pub environment_id: Option<i64>,
    pub session_id: i64,
    pub source_fragment_id: i64,
    pub region_key: String,
    pub region_kind: RegionKind,
    pub support_point_count: i64,
    pub confidence: f64,
    pub centroid_json: String,
    pub bounds_json: String,
    pub signature_hash: String,
    pub metadata_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RegionLinkUpsert {
    pub left_region_id: i64,
    pub right_region_id: i64,
    pub link_kind: LinkKind,
    pub score: f64,
    pub relative_transform_json: String,
    pub contradiction_score: f64,
    pub state: CandidateState,
    pub evidence_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionMergeCandidateUpsert {
    pub left_session_id: i64,
    pub right_session_id: i64,
    pub left_region_id: i64,
    pub right_region_id: i64,
    pub candidate_kind: LinkKind,
    pub score: f64,
    pub transform_consistency: f64,
    pub contradiction_score: f64,
    pub state: CandidateState,
    pub reason_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MergeCandidateThresholds {
    pub overlap_min: f64,
    pub transform_consistency_min: f64,
    pub contradiction_max: f64,
}

impl Default for MergeCandidateThresholds {
    fn default() -> Self {
        Self {
            overlap_min: 0.55,
            transform_consistency_min: 0.60,
            contradiction_max: 0.35,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MergeCandidateGenerationReport {
    pub evaluated_pairs: usize,
    pub persisted_candidates: usize,
    pub persisted_region_links: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlanRunUpsert {
    pub environment_id: i64,
    pub session_id: Option<i64>,
    pub source_region_id: Option<i64>,
    pub target_region_id: Option<i64>,
    pub planner_kind: String,
    pub status: PlanStatus,
    pub path_cost: f64,
    pub risk_score: f64,
    pub clearance_min_ft: f64,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub summary_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlanRunRegionRow {
    pub plan_run_id: i64,
    pub step_index: i64,
    pub region_id: i64,
    pub cumulative_cost: f64,
    pub cumulative_risk: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlanRunRegionUpsert {
    pub step_index: i64,
    pub region_id: i64,
    pub cumulative_cost: f64,
    pub cumulative_risk: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlannerSnapshotRow {
    pub id: i64,
    pub plan_run_id: i64,
    pub session_id: i64,
    pub graph_fragment_id: Option<i64>,
    pub snapshot_key: String,
    pub status: String,
    pub window_start_sec: Option<f64>,
    pub window_end_sec: Option<f64>,
    pub selected_candidate_key: Option<String>,
    pub trace_key: Option<String>,
    pub trace_status: String,
    pub summary_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlannerSnapshotUpsert {
    pub plan_run_id: i64,
    pub session_id: i64,
    pub graph_fragment_id: Option<i64>,
    pub snapshot_key: String,
    pub status: String,
    pub window_start_sec: Option<f64>,
    pub window_end_sec: Option<f64>,
    pub selected_candidate_key: Option<String>,
    pub trace_key: Option<String>,
    pub trace_status: String,
    pub summary_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrajectoryCandidateRow {
    pub id: i64,
    pub planner_snapshot_id: i64,
    pub candidate_key: String,
    pub status: String,
    pub score: f64,
    pub world_region_keys_json: String,
    pub rejection_reason: Option<String>,
    pub summary_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrajectoryCandidateUpsert {
    pub candidate_key: String,
    pub status: String,
    pub score: f64,
    pub world_region_keys_json: String,
    pub rejection_reason: Option<String>,
    pub summary_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GateReviewEntryUpsert {
    pub id: Option<i64>,
    pub session_id: i64,
    pub gate_number: i64,
    pub gate_label: String,
    pub track_section: String,
    pub frame_index: Option<i64>,
    pub timestamp_sec: Option<f64>,
    pub confidence_score: f64,
    pub pass_order: Option<i64>,
    pub status: String,
    pub source: String,
    pub note: String,
    pub summary_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MissionPortfolioUpsert {
    pub portfolio_key: String,
    pub mission_type: String,
    pub mission_subtype: String,
    pub display_name: String,
    pub description: String,
    pub rubric_json: String,
    pub metadata_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MissionInstanceUpsert {
    pub portfolio_id: i64,
    pub session_id: Option<i64>,
    pub instance_key: String,
    pub title: String,
    pub status: String,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub objective_summary: String,
    pub summary_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MissionPhaseUpsert {
    pub mission_instance_id: i64,
    pub phase_key: String,
    pub phase_kind: String,
    pub window_start_sec: f64,
    pub window_end_sec: Option<f64>,
    pub status: String,
    pub summary_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OutcomeAssessmentUpsert {
    pub mission_instance_id: i64,
    pub phase_id: Option<i64>,
    pub assessment_status: String,
    pub overall_result: String,
    pub scorecard_json: String,
    pub evidence_json: String,
    pub recommended_plan_revision: String,
    pub assessor: String,
    pub assessed_at: String,
    pub summary_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeliberatePlanThresholds {
    pub free_space_confidence_min: f64,
    pub risk_threshold: f64,
    pub max_edge_distance_ft: f64,
}

impl Default for DeliberatePlanThresholds {
    fn default() -> Self {
        Self {
            free_space_confidence_min: 0.65,
            risk_threshold: 0.70,
            max_edge_distance_ft: 25.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeliberatePlanRequest {
    pub environment_id: i64,
    pub planner_kind: String,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub source_region_id: i64,
    pub target_region_id: i64,
    pub thresholds: DeliberatePlanThresholds,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionViewSummaryRow {
    pub session_id: i64,
    pub analysis_jobs: i64,
    pub graph_fragments: i64,
    pub world_regions: i64,
    pub plan_runs: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionEnvironmentViewRow {
    pub session_summary: SessionViewSummaryRow,
    pub world_regions: Vec<WorldRegionRow>,
    pub plan_runs: Vec<PlanRunRow>,
}

pub struct SessionStore {
    connection: Connection,
}

impl SessionStore {
    pub fn open(path: &str) -> rusqlite::Result<Self> {
        let connection = Connection::open(path)?;
        let store = Self { connection };
        store.init_schema()?;
        Ok(store)
    }

    pub fn open_in_memory() -> rusqlite::Result<Self> {
        let connection = Connection::open_in_memory()?;
        let store = Self { connection };
        store.init_schema()?;
        Ok(store)
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    /// Create or migrate the schema.
    ///
    /// A database whose `user_version` is higher than [`SCHEMA_VERSION`] was
    /// written by a newer build; it is refused instead of being silently
    /// rewritten. Unstamped databases (`user_version = 0`), including ones
    /// created before the store tracked a revision, are brought forward in
    /// place: every statement is `IF NOT EXISTS`, so existing rows survive.
    pub fn init_schema(&self) -> rusqlite::Result<()> {
        let version: i64 =
            self.connection
                .query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version > SCHEMA_VERSION {
            return Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_ERROR),
                Some(format!(
                    "session store schema version {version} is newer than the supported version {SCHEMA_VERSION}"
                )),
            ));
        }

        self.connection.execute_batch(SCHEMA_SQL)?;

        if version < SCHEMA_VERSION {
            self.connection
                .execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))?;
        }
        Ok(())
    }
}

const SCHEMA_SQL: &str = r#"
PRAGMA journal_mode=WAL;
PRAGMA foreign_keys=ON;

CREATE TABLE IF NOT EXISTS sessions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    path TEXT NOT NULL UNIQUE,
    filename TEXT NOT NULL,
    media_kind TEXT NOT NULL DEFAULT 'unknown',
    analysis_kind TEXT NOT NULL DEFAULT 'pending',
    status TEXT NOT NULL DEFAULT 'ready',
    duration_sec REAL NOT NULL DEFAULT 0,
    imported_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS session_streams (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id INTEGER NOT NULL,
    stream_key TEXT NOT NULL,
    stream_kind TEXT NOT NULL,
    role TEXT NOT NULL DEFAULT 'observation',
    path TEXT NOT NULL,
    sync_group TEXT NOT NULL DEFAULT 'default',
    metadata_json TEXT NOT NULL DEFAULT '{}',
    UNIQUE(session_id, stream_key),
    FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS abstraction_epochs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id INTEGER NOT NULL,
    epoch_index INTEGER NOT NULL,
    layer_name TEXT NOT NULL,
    abstraction_family TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'ready',
    created_at TEXT NOT NULL,
    completed_at TEXT,
    summary_json TEXT NOT NULL DEFAULT '{}',
    UNIQUE(session_id, epoch_index, layer_name, abstraction_family),
    FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS abstraction_spaces (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    epoch_id INTEGER NOT NULL,
    space_family TEXT NOT NULL,
    abstraction_name TEXT NOT NULL,
    source_kind TEXT NOT NULL,
    representation_kind TEXT NOT NULL,
    uncertainty_kind TEXT NOT NULL,
    dimensionality INTEGER NOT NULL DEFAULT 0,
    schema_json TEXT NOT NULL DEFAULT '{}',
    UNIQUE(epoch_id, abstraction_name),
    FOREIGN KEY(epoch_id) REFERENCES abstraction_epochs(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS abstract_state_samples (
    epoch_id INTEGER NOT NULL,
    space_id INTEGER NOT NULL,
    step INTEGER NOT NULL,
    timestamp_sec REAL NOT NULL,
    symbol_key TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    confidence REAL NOT NULL DEFAULT 1.0,
    sample_hash TEXT NOT NULL,
    PRIMARY KEY(epoch_id, space_id, step),
    FOREIGN KEY(epoch_id) REFERENCES abstraction_epochs(id) ON DELETE CASCADE,
    FOREIGN KEY(space_id) REFERENCES abstraction_spaces(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS analysis_jobs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id INTEGER NOT NULL,
    environment_id INTEGER,
    job_kind TEXT NOT NULL,
    status TEXT NOT NULL,
    requested_at TEXT NOT NULL,
    started_at TEXT,
    completed_at TEXT,
    window_start_sec REAL,
    window_end_sec REAL,
    spec_json TEXT NOT NULL DEFAULT '{}',
    summary_json TEXT NOT NULL DEFAULT '{}',
    failure_json TEXT,
    UNIQUE(session_id, job_kind, requested_at),
    FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS graph_fragments (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    analysis_job_id INTEGER NOT NULL,
    session_id INTEGER NOT NULL,
    epoch_id INTEGER,
    stream_key TEXT NOT NULL,
    fragment_key TEXT NOT NULL,
    graph_kind TEXT NOT NULL,
    graph_form TEXT NOT NULL,
    exactness TEXT NOT NULL,
    variable_count INTEGER NOT NULL,
    factor_count INTEGER NOT NULL,
    tree_width INTEGER,
    root_variable_key TEXT,
    window_start_sec REAL NOT NULL,
    window_end_sec REAL NOT NULL,
    summary_json TEXT NOT NULL DEFAULT '{}',
    UNIQUE(analysis_job_id, fragment_key),
    FOREIGN KEY(analysis_job_id) REFERENCES analysis_jobs(id) ON DELETE CASCADE,
    FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE,
    FOREIGN KEY(epoch_id) REFERENCES abstraction_epochs(id) ON DELETE SET NULL
);

CREATE TABLE IF NOT EXISTS message_snapshots (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    fragment_id INTEGER NOT NULL,
    edge_key TEXT NOT NULL,
    direction TEXT NOT NULL,
    iteration_index INTEGER NOT NULL,
    source_node_key TEXT NOT NULL,
    target_node_key TEXT NOT NULL,
    values_json TEXT NOT NULL,
    residual_norm REAL NOT NULL,
    message_hash TEXT NOT NULL,
    UNIQUE(fragment_id, edge_key, direction, iteration_index),
    FOREIGN KEY(fragment_id) REFERENCES graph_fragments(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS belief_matrices (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    fragment_id INTEGER NOT NULL,
    variable_key TEXT NOT NULL,
    domain_kind TEXT NOT NULL,
    normalization_error REAL NOT NULL,
    entropy REAL NOT NULL,
    max_state_key TEXT NOT NULL,
    values_json TEXT NOT NULL,
    matrix_hash TEXT NOT NULL,
    UNIQUE(fragment_id, variable_key),
    FOREIGN KEY(fragment_id) REFERENCES graph_fragments(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS world_regions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    environment_id INTEGER,
    session_id INTEGER NOT NULL,
    source_fragment_id INTEGER NOT NULL,
    region_key TEXT NOT NULL,
    region_kind TEXT NOT NULL,
    support_point_count INTEGER NOT NULL,
    confidence REAL NOT NULL,
    centroid_json TEXT NOT NULL,
    bounds_json TEXT NOT NULL,
    signature_hash TEXT NOT NULL,
    metadata_json TEXT NOT NULL DEFAULT '{}',
    UNIQUE(session_id, region_key),
    FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE,
    FOREIGN KEY(source_fragment_id) REFERENCES graph_fragments(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS region_links (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    left_region_id INTEGER NOT NULL,
    right_region_id INTEGER NOT NULL,
    link_kind TEXT NOT NULL,
    score REAL NOT NULL,
    relative_transform_json TEXT NOT NULL,
    contradiction_score REAL NOT NULL,
    state TEXT NOT NULL,
    evidence_json TEXT NOT NULL DEFAULT '{}',
    UNIQUE(left_region_id, right_region_id, link_kind),
    FOREIGN KEY(left_region_id) REFERENCES world_regions(id) ON DELETE CASCADE,
    FOREIGN KEY(right_region_id) REFERENCES world_regions(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS session_merge_candidates (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    left_session_id INTEGER NOT NULL,
    right_session_id INTEGER NOT NULL,
    left_region_id INTEGER NOT NULL,
    right_region_id INTEGER NOT NULL,
    candidate_kind TEXT NOT NULL,
    score REAL NOT NULL,
    transform_consistency REAL NOT NULL,
    contradiction_score REAL NOT NULL,
    state TEXT NOT NULL,
    reason_json TEXT NOT NULL DEFAULT '{}',
    UNIQUE(left_session_id, right_session_id, left_region_id, right_region_id, candidate_kind),
    FOREIGN KEY(left_session_id) REFERENCES sessions(id) ON DELETE CASCADE,
    FOREIGN KEY(right_session_id) REFERENCES sessions(id) ON DELETE CASCADE,
    FOREIGN KEY(left_region_id) REFERENCES world_regions(id) ON DELETE CASCADE,
    FOREIGN KEY(right_region_id) REFERENCES world_regions(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS plan_runs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    environment_id INTEGER NOT NULL,
    session_id INTEGER,
    source_region_id INTEGER,
    target_region_id INTEGER,
    planner_kind TEXT NOT NULL,
    status TEXT NOT NULL,
    path_cost REAL NOT NULL,
    risk_score REAL NOT NULL,
    clearance_min_ft REAL NOT NULL,
    started_at TEXT NOT NULL,
    completed_at TEXT,
    summary_json TEXT NOT NULL DEFAULT '{}',
    UNIQUE(environment_id, planner_kind, started_at),
    FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE SET NULL,
    FOREIGN KEY(source_region_id) REFERENCES world_regions(id) ON DELETE SET NULL,
    FOREIGN KEY(target_region_id) REFERENCES world_regions(id) ON DELETE SET NULL
);

CREATE TABLE IF NOT EXISTS plan_run_regions (
    plan_run_id INTEGER NOT NULL,
    step_index INTEGER NOT NULL,
    region_id INTEGER NOT NULL,
    cumulative_cost REAL NOT NULL,
    cumulative_risk REAL NOT NULL,
    PRIMARY KEY(plan_run_id, step_index),
    FOREIGN KEY(plan_run_id) REFERENCES plan_runs(id) ON DELETE CASCADE,
    FOREIGN KEY(region_id) REFERENCES world_regions(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS planner_snapshots (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    plan_run_id INTEGER NOT NULL,
    session_id INTEGER NOT NULL,
    graph_fragment_id INTEGER,
    snapshot_key TEXT NOT NULL,
    status TEXT NOT NULL,
    window_start_sec REAL,
    window_end_sec REAL,
    selected_candidate_key TEXT,
    trace_key TEXT,
    trace_status TEXT NOT NULL DEFAULT 'executed',
    summary_json TEXT NOT NULL DEFAULT '{}',
    UNIQUE(plan_run_id, snapshot_key),
    FOREIGN KEY(plan_run_id) REFERENCES plan_runs(id) ON DELETE CASCADE,
    FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE,
    FOREIGN KEY(graph_fragment_id) REFERENCES graph_fragments(id) ON DELETE SET NULL
);

CREATE INDEX IF NOT EXISTS planner_snapshots_session_idx
    ON planner_snapshots (session_id, window_start_sec ASC, window_end_sec ASC, id ASC);
CREATE INDEX IF NOT EXISTS planner_snapshots_plan_run_idx
    ON planner_snapshots (plan_run_id, id ASC);

CREATE TABLE IF NOT EXISTS trajectory_candidates (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    planner_snapshot_id INTEGER NOT NULL,
    candidate_key TEXT NOT NULL,
    status TEXT NOT NULL,
    score REAL NOT NULL,
    world_region_keys_json TEXT NOT NULL DEFAULT '[]',
    rejection_reason TEXT,
    summary_json TEXT NOT NULL DEFAULT '{}',
    UNIQUE(planner_snapshot_id, candidate_key),
    FOREIGN KEY(planner_snapshot_id) REFERENCES planner_snapshots(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS trajectory_candidates_snapshot_idx
    ON trajectory_candidates (planner_snapshot_id, id ASC);

CREATE TABLE IF NOT EXISTS coach_scenario_reviews (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    environment_id INTEGER NOT NULL,
    plan_run_id INTEGER,
    generated_at TEXT NOT NULL,
    risk_score REAL NOT NULL,
    confidence_score REAL NOT NULL,
    summary_json TEXT NOT NULL DEFAULT '{}',
    UNIQUE(environment_id, plan_run_id, generated_at),
    FOREIGN KEY(plan_run_id) REFERENCES plan_runs(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS gate_review_entries (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id INTEGER NOT NULL,
    gate_number INTEGER NOT NULL,
    gate_label TEXT NOT NULL DEFAULT '',
    track_section TEXT NOT NULL DEFAULT '',
    frame_index INTEGER,
    timestamp_sec REAL,
    confidence_score REAL NOT NULL DEFAULT 0.0,
    pass_order INTEGER,
    status TEXT NOT NULL DEFAULT 'observed',
    source TEXT NOT NULL DEFAULT 'manual',
    note TEXT NOT NULL DEFAULT '',
    summary_json TEXT NOT NULL DEFAULT '{}',
    FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS gate_review_entries_session_idx
    ON gate_review_entries (session_id, gate_number ASC, pass_order ASC, timestamp_sec ASC, id ASC);

CREATE TABLE IF NOT EXISTS mission_portfolios (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    portfolio_key TEXT NOT NULL UNIQUE,
    mission_type TEXT NOT NULL,
    mission_subtype TEXT NOT NULL DEFAULT '',
    display_name TEXT NOT NULL DEFAULT '',
    description TEXT NOT NULL DEFAULT '',
    rubric_json TEXT NOT NULL DEFAULT '{}',
    metadata_json TEXT NOT NULL DEFAULT '{}'
);

CREATE INDEX IF NOT EXISTS mission_portfolios_type_idx
    ON mission_portfolios (mission_type, mission_subtype, id ASC);

CREATE TABLE IF NOT EXISTS mission_instances (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    portfolio_id INTEGER NOT NULL,
    session_id INTEGER,
    instance_key TEXT NOT NULL,
    title TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'draft',
    started_at TEXT NOT NULL,
    completed_at TEXT,
    objective_summary TEXT NOT NULL DEFAULT '',
    summary_json TEXT NOT NULL DEFAULT '{}',
    UNIQUE(portfolio_id, instance_key),
    FOREIGN KEY(portfolio_id) REFERENCES mission_portfolios(id) ON DELETE CASCADE,
    FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE SET NULL
);

CREATE INDEX IF NOT EXISTS mission_instances_session_idx
    ON mission_instances (session_id, started_at ASC, id ASC);

CREATE TABLE IF NOT EXISTS mission_phases (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    mission_instance_id INTEGER NOT NULL,
    phase_key TEXT NOT NULL,
    phase_kind TEXT NOT NULL,
    window_start_sec REAL NOT NULL,
    window_end_sec REAL,
    status TEXT NOT NULL DEFAULT 'planned',
    summary_json TEXT NOT NULL DEFAULT '{}',
    UNIQUE(mission_instance_id, phase_key),
    FOREIGN KEY(mission_instance_id) REFERENCES mission_instances(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS mission_phases_instance_idx
    ON mission_phases (mission_instance_id, window_start_sec ASC, id ASC);

CREATE TABLE IF NOT EXISTS outcome_assessments (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    mission_instance_id INTEGER NOT NULL,
    phase_id INTEGER,
    assessment_status TEXT NOT NULL DEFAULT 'draft',
    overall_result TEXT NOT NULL DEFAULT 'partial_success',
    scorecard_json TEXT NOT NULL DEFAULT '{}',
    evidence_json TEXT NOT NULL DEFAULT '[]',
    recommended_plan_revision TEXT NOT NULL DEFAULT '',
    assessor TEXT NOT NULL DEFAULT '',
    assessed_at TEXT NOT NULL,
    summary_json TEXT NOT NULL DEFAULT '{}',
    UNIQUE(mission_instance_id, phase_id, assessor, assessed_at),
    FOREIGN KEY(mission_instance_id) REFERENCES mission_instances(id) ON DELETE CASCADE,
    FOREIGN KEY(phase_id) REFERENCES mission_phases(id) ON DELETE SET NULL
);

CREATE INDEX IF NOT EXISTS outcome_assessments_instance_idx
    ON outcome_assessments (mission_instance_id, assessed_at DESC, id DESC);

CREATE TABLE IF NOT EXISTS sync_ops (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    op_id TEXT NOT NULL UNIQUE,
    schema_version TEXT NOT NULL,
    replica_id TEXT NOT NULL,
    replica_role TEXT NOT NULL,
    run_id TEXT NOT NULL,
    counter INTEGER NOT NULL,
    timestamp_hlc TEXT NOT NULL,
    namespace TEXT NOT NULL,
    key TEXT NOT NULL,
    body_json TEXT NOT NULL,
    status TEXT NOT NULL,
    status_detail TEXT NOT NULL DEFAULT '',
    received_at TEXT NOT NULL,
    UNIQUE(replica_id, run_id, counter)
);

CREATE INDEX IF NOT EXISTS sync_ops_seq_idx
    ON sync_ops (seq ASC);
CREATE INDEX IF NOT EXISTS sync_ops_namespace_idx
    ON sync_ops (namespace, key, seq ASC);

CREATE TABLE IF NOT EXISTS sync_materialized_states (
    namespace TEXT NOT NULL,
    key TEXT NOT NULL,
    state_json TEXT NOT NULL,
    updated_hlc TEXT NOT NULL,
    last_op_id TEXT NOT NULL,
    PRIMARY KEY(namespace, key)
);

CREATE INDEX IF NOT EXISTS sync_materialized_states_namespace_idx
    ON sync_materialized_states (namespace, key);

CREATE TABLE IF NOT EXISTS sync_append_entries (
    namespace TEXT NOT NULL,
    key TEXT NOT NULL,
    entry_id TEXT NOT NULL,
    value_json TEXT NOT NULL,
    timestamp_hlc TEXT NOT NULL,
    source_op_id TEXT NOT NULL UNIQUE,
    replica_id TEXT NOT NULL,
    PRIMARY KEY(namespace, key, entry_id)
);

CREATE INDEX IF NOT EXISTS sync_append_entries_namespace_idx
    ON sync_append_entries (namespace, key, timestamp_hlc ASC);

CREATE TABLE IF NOT EXISTS sync_replicas (
    replica_id TEXT PRIMARY KEY,
    replica_role TEXT NOT NULL,
    display_name TEXT NOT NULL DEFAULT '',
    endpoint TEXT NOT NULL DEFAULT '',
    trust_state TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    capabilities_json TEXT NOT NULL DEFAULT '{}',
    metadata_json TEXT NOT NULL DEFAULT '{}',
    last_seen_hlc TEXT,
    last_seen_at TEXT,
    last_sync_seq INTEGER NOT NULL DEFAULT 0,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS sync_peer_cursors (
    peer_replica_id TEXT PRIMARY KEY,
    remote_seq INTEGER NOT NULL DEFAULT 0,
    local_applied_seq INTEGER NOT NULL DEFAULT 0,
    remote_latest_seq INTEGER NOT NULL DEFAULT 0,
    updated_at TEXT NOT NULL,
    FOREIGN KEY(peer_replica_id) REFERENCES sync_replicas(replica_id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS world_model_proposals (
    proposal_id TEXT PRIMARY KEY,
    proposal_kind TEXT NOT NULL,
    source_replica_id TEXT NOT NULL,
    source_replica_role TEXT NOT NULL,
    created_at_hlc TEXT NOT NULL,
    status TEXT NOT NULL,
    belief_weight REAL NOT NULL,
    source_weight REAL NOT NULL,
    mission_relevance REAL NOT NULL,
    confidence REAL NOT NULL,
    lineage_json TEXT NOT NULL DEFAULT '{}',
    body_json TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS world_model_proposals_kind_status_idx
    ON world_model_proposals (proposal_kind, status, created_at_hlc ASC);

CREATE TABLE IF NOT EXISTS world_model_decisions (
    decision_id TEXT PRIMARY KEY,
    decision_kind TEXT NOT NULL,
    curator_replica_id TEXT NOT NULL,
    curator_replica_role TEXT NOT NULL,
    created_at_hlc TEXT NOT NULL,
    target_proposal_ids_json TEXT NOT NULL,
    output_ids_json TEXT NOT NULL DEFAULT '[]',
    reason TEXT NOT NULL DEFAULT ''
);

CREATE INDEX IF NOT EXISTS world_model_decisions_kind_idx
    ON world_model_decisions (decision_kind, created_at_hlc ASC);

CREATE TABLE IF NOT EXISTS world_model_canonical (
    canonical_id TEXT PRIMARY KEY,
    canonical_kind TEXT NOT NULL,
    source_decision_id TEXT NOT NULL,
    source_proposal_ids_json TEXT NOT NULL,
    accepted_at_hlc TEXT NOT NULL,
    status TEXT NOT NULL,
    body_json TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS world_model_canonical_kind_idx
    ON world_model_canonical (canonical_kind, accepted_at_hlc ASC);

CREATE TABLE IF NOT EXISTS world_model_tensor_state (
    state_key TEXT PRIMARY KEY,
    state_kind TEXT NOT NULL,
    profile_id TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    updated_at_hlc TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS world_model_tensor_state_kind_idx
    ON world_model_tensor_state (state_kind, updated_at_hlc ASC);
"#;
