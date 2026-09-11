//! Behavioural tests for the persistent session store, run against real
//! temporary SQLite databases (never `:memory:`) so that reopen and on-disk
//! schema behaviour are exercised.

use qualia_session_store::mission_types::{
    AnalysisJobKind, AnalysisJobStatus, CandidateState, ExactnessKind, GraphForm, GraphKind,
    LinkKind, PlanStatus, RegionKind,
};
use qualia_session_store::{
    AbstractStateSample, AbstractionEpochUpsert, AbstractionSpaceUpsert, AnalysisJobUpsert,
    DeliberatePlanRequest, DeliberatePlanThresholds, GraphFragmentUpsert, MergeCandidateThresholds,
    MissionInstanceUpsert, MissionPhaseUpsert, MissionPortfolioUpsert, OutcomeAssessmentUpsert,
    SessionStore, SessionUpsert, WorldRegionUpsert,
};
use rusqlite::Connection;
use tempfile::TempDir;

fn database_path(dir: &TempDir) -> String {
    dir.path()
        .join("qualia-sessions.sqlite")
        .to_string_lossy()
        .into_owned()
}

fn session(path: &str) -> SessionUpsert {
    SessionUpsert {
        path: path.to_string(),
        filename: "flight-01.mp4".to_string(),
        media_kind: "flight_capture".to_string(),
        analysis_kind: "graph_inference".to_string(),
        status: "ready".to_string(),
        duration_sec: 12.5,
        imported_at: "2026-09-11T00:00:00Z".to_string(),
    }
}

#[test]
fn open_stamps_the_schema_version_and_reopens_cleanly() {
    let dir = TempDir::new().expect("temp dir");
    let path = database_path(&dir);

    {
        let store = SessionStore::open(&path).expect("first open");
        let version: i64 = store
            .connection()
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("schema version");
        assert_eq!(version, 1);
    }

    let reopened = SessionStore::open(&path).expect("second open");
    let sessions_table: i64 = reopened
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='sessions'",
            [],
            |row| row.get(0),
        )
        .expect("sessions table");
    assert_eq!(sessions_table, 1);
    assert!(reopened.list_sessions().expect("sessions").is_empty());
}

#[test]
fn session_round_trips_across_a_reopen() {
    let dir = TempDir::new().expect("temp dir");
    let path = database_path(&dir);
    let expected = session("/tmp/flight-01.mp4");

    let session_id = {
        let store = SessionStore::open(&path).expect("open store");
        let id = store.upsert_session(&expected).expect("insert session");
        assert!(id > 0);

        let listed = store.list_sessions().expect("list sessions");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].path, expected.path);
        assert_eq!(listed[0].filename, expected.filename);
        assert_eq!(listed[0].duration_sec, expected.duration_sec);

        id
    };

    let reopened = SessionStore::open(&path).expect("reopen store");
    let stored = reopened
        .session_by_id(session_id)
        .expect("session by id")
        .expect("row present after reopen");
    assert_eq!(stored.id, session_id);
    assert_eq!(stored.path, expected.path);
    assert_eq!(stored.media_kind, expected.media_kind);
    assert_eq!(stored.analysis_kind, expected.analysis_kind);
    assert_eq!(
        reopened.latest_session().expect("latest").map(|row| row.id),
        Some(session_id)
    );
}

#[test]
fn mission_rows_read_back_with_the_same_id_and_outcome() {
    let dir = TempDir::new().expect("temp dir");
    let path = database_path(&dir);
    let store = SessionStore::open(&path).expect("open store");
    let session_id = store.upsert_session(&session("/tmp/mission.mp4")).expect("session");

    let portfolio_id = store
        .upsert_mission_portfolio(&MissionPortfolioUpsert {
            portfolio_key: "yard-patrol".to_string(),
            mission_type: "inspection".to_string(),
            mission_subtype: "perimeter".to_string(),
            display_name: "Yard patrol".to_string(),
            description: "Walk the fence line and report obstructions".to_string(),
            rubric_json: r#"{"coverage":0.8}"#.to_string(),
            metadata_json: "{}".to_string(),
        })
        .expect("portfolio");

    let instance_id = store
        .upsert_mission_instance(&MissionInstanceUpsert {
        portfolio_id,
        session_id: Some(session_id),
        instance_key: "run-0001".to_string(),
        title: "Run 0001".to_string(),
        status: "running".to_string(),
        started_at: "2026-09-11T00:01:00Z".to_string(),
        completed_at: None,
        objective_summary: "first attempt".to_string(),
        summary_json: "{}".to_string(),
    })
        .expect("instance");

    let phase_id = store
        .upsert_mission_phase(&MissionPhaseUpsert {
            mission_instance_id: instance_id,
            phase_key: "approach".to_string(),
            phase_kind: "transit".to_string(),
            window_start_sec: 0.0,
            window_end_sec: Some(4.5),
            status: "complete".to_string(),
            summary_json: "{}".to_string(),
        })
        .expect("phase");

    let assessment_id = store
        .upsert_outcome_assessment(&OutcomeAssessmentUpsert {
            mission_instance_id: instance_id,
            phase_id: Some(phase_id),
            assessment_status: "final".to_string(),
            overall_result: "success".to_string(),
            scorecard_json: r#"{"coverage":0.91}"#.to_string(),
            evidence_json: "[]".to_string(),
            recommended_plan_revision: "keep".to_string(),
            assessor: "coach".to_string(),
            assessed_at: "2026-09-11T00:02:00Z".to_string(),
            summary_json: "{}".to_string(),
        })
        .expect("assessment");

    let instances = store.list_mission_instances(portfolio_id).expect("instances");
    assert_eq!(instances.len(), 1);
    assert_eq!(instances[0].id, instance_id);
    assert_eq!(instances[0].session_id, Some(session_id));
    assert_eq!(instances[0].instance_key, "run-0001");

    let phases = store.list_mission_phases(instance_id).expect("phases");
    assert_eq!(phases.len(), 1);
    assert_eq!(phases[0].id, phase_id);
    assert_eq!(phases[0].phase_key, "approach");

    let assessments = store
        .list_outcome_assessments(instance_id)
        .expect("assessments");
    assert_eq!(assessments.len(), 1);
    assert_eq!(assessments[0].id, assessment_id);
    assert_eq!(assessments[0].overall_result, "success");
    assert_eq!(assessments[0].phase_id, Some(phase_id));
    assert_eq!(assessments[0].assessor, "coach");

    // The instance write is an upsert on `(portfolio_id, instance_key)`, so a
    // second write with the same key must update the existing row.
    let updated_id = store
        .upsert_mission_instance(&MissionInstanceUpsert {
            portfolio_id,
            session_id: Some(session_id),
            instance_key: "run-0001".to_string(),
            title: "Run 0001".to_string(),
            status: "complete".to_string(),
            started_at: "2026-09-11T00:01:00Z".to_string(),
            completed_at: Some("2026-09-11T00:02:30Z".to_string()),
            objective_summary: "first attempt".to_string(),
            summary_json: "{}".to_string(),
        })
        .expect("update instance");
    assert_eq!(updated_id, instance_id);
    let instances = store.list_mission_instances(portfolio_id).expect("instances");
    assert_eq!(instances.len(), 1);
    assert_eq!(instances[0].status, "complete");
}

#[test]
fn epoch_space_and_sample_queries_return_what_was_inserted() {
    let dir = TempDir::new().expect("temp dir");
    let path = database_path(&dir);
    let store = SessionStore::open(&path).expect("open store");
    let session_id = store.upsert_session(&session("/tmp/epochs.mp4")).expect("session");

    let epoch_id = store
        .upsert_epoch(&AbstractionEpochUpsert {
            session_id,
            epoch_index: 3,
            layer_name: "l2".to_string(),
            abstraction_family: "spatial".to_string(),
            status: "complete".to_string(),
            created_at: "2026-09-11T00:03:00Z".to_string(),
            completed_at: Some("2026-09-11T00:04:00Z".to_string()),
            summary_json: "{}".to_string(),
        })
        .expect("epoch");
    assert!(epoch_id > 0);

    let space_id = store
        .upsert_space(&AbstractionSpaceUpsert {
            epoch_id,
            space_family: "occupancy".to_string(),
            abstraction_name: "voxel_8".to_string(),
            source_kind: "lidar".to_string(),
            representation_kind: "grid".to_string(),
            uncertainty_kind: "logit".to_string(),
            dimensionality: 3,
            schema_json: "{}".to_string(),
        })
        .expect("space");

    let samples: Vec<AbstractStateSample> = (0..5)
        .map(|step| AbstractStateSample {
            step,
            timestamp_sec: step as f64 * 0.1,
            symbol_key: format!("cell-{step}"),
            payload_json: "{}".to_string(),
            confidence: 0.9,
            sample_hash: format!("hash-{step}"),
        })
        .collect();
    store
        .replace_state_samples(epoch_id, space_id, &samples)
        .expect("samples");

    let epochs = store.list_epochs(session_id).expect("epochs");
    assert_eq!(epochs.len(), 1);
    assert_eq!(epochs[0].id, epoch_id);
    assert_eq!(epochs[0].abstraction_family, "spatial");
    assert_eq!(epochs[0].completed_at.as_deref(), Some("2026-09-11T00:04:00Z"));

    let spaces = store.list_spaces(epoch_id).expect("spaces");
    assert_eq!(spaces.len(), 1);
    assert_eq!(spaces[0].id, space_id);
    assert_eq!(spaces[0].abstraction_name, "voxel_8");
    assert_eq!(spaces[0].dimensionality, 3);

    let all = store
        .list_state_samples(space_id, None, None, None)
        .expect("all samples");
    assert_eq!(all.len(), 5);
    assert_eq!(all[0].symbol_key, "cell-0");
    assert_eq!(all[4].epoch_id, epoch_id);

    let window = store
        .list_state_samples(space_id, Some(1), Some(3), None)
        .expect("windowed samples");
    assert_eq!(
        window.iter().map(|row| row.step).collect::<Vec<_>>(),
        vec![1, 2, 3]
    );

    let limited = store
        .list_state_samples(space_id, None, None, Some(2))
        .expect("limited samples");
    assert_eq!(limited.len(), 2);

    // Replacing the sample set must drop the previous rows, not append.
    store
        .replace_state_samples(epoch_id, space_id, &samples[..2])
        .expect("replace");
    assert_eq!(
        store
            .list_state_samples(space_id, None, None, None)
            .expect("after replace")
            .len(),
        2
    );
}

#[test]
fn analysis_job_upserts_on_session_kind_and_timestamp() {
    let dir = TempDir::new().expect("temp dir");
    let path = database_path(&dir);
    let store = SessionStore::open(&path).expect("open store");
    let session_id = store.upsert_session(&session("/tmp/jobs.mp4")).expect("session");

    let job = AnalysisJobUpsert {
        session_id,
        environment_id: None,
        job_kind: AnalysisJobKind::GraphInference,
        status: AnalysisJobStatus::Queued,
        requested_at: "2026-09-11T00:05:00Z".to_string(),
        started_at: None,
        completed_at: None,
        window_start_sec: Some(0.0),
        window_end_sec: Some(2.0),
        spec_json: "{}".to_string(),
        summary_json: "{}".to_string(),
        failure_json: None,
    };
    let job_id = store.upsert_analysis_job(&job).expect("insert job");

    let updated_id = store
        .upsert_analysis_job(&AnalysisJobUpsert {
            status: AnalysisJobStatus::Complete,
            summary_json: r#"{"fragments":2}"#.to_string(),
            ..job
        })
        .expect("update job");
    assert_eq!(updated_id, job_id);

    let jobs = store.list_analysis_jobs(session_id).expect("jobs");
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].id, job_id);
    assert_eq!(jobs[0].status, AnalysisJobStatus::Complete);
    assert_eq!(jobs[0].job_kind, AnalysisJobKind::GraphInference);
    assert_eq!(jobs[0].summary_json, r#"{"fragments":2}"#);
}

#[test]
fn unknown_schema_version_is_rejected_without_touching_the_database() {
    let dir = TempDir::new().expect("temp dir");
    let path = database_path(&dir);

    {
        let connection = Connection::open(&path).expect("raw open");
        connection
            .execute_batch(
                "CREATE TABLE legacy_marker (value INTEGER NOT NULL);\nPRAGMA user_version = 99;",
            )
            .expect("stamp unknown version");
    }

    let error = SessionStore::open(&path)
        .err()
        .expect("unknown version must be refused");
    let message = format!("{error}");
    assert!(
        message.contains("99"),
        "error should name the offending version, got: {message}"
    );

    let connection = Connection::open(&path).expect("raw reopen");
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("version");
    assert_eq!(version, 99, "refusing to open must not rewrite the stamp");
    let tables: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )
        .expect("table count");
    assert_eq!(
        tables, 1,
        "refusing to open must not run the migration batch"
    );
}

#[test]
fn unstamped_legacy_database_gains_the_full_schema_in_place() {
    let dir = TempDir::new().expect("temp dir");
    let path = database_path(&dir);

    {
        let connection = Connection::open(&path).expect("raw open");
        connection
            .execute_batch(
                r#"
                CREATE TABLE sessions (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    path TEXT NOT NULL UNIQUE,
                    filename TEXT NOT NULL,
                    media_kind TEXT NOT NULL DEFAULT 'unknown',
                    analysis_kind TEXT NOT NULL DEFAULT 'pending',
                    status TEXT NOT NULL DEFAULT 'ready',
                    duration_sec REAL NOT NULL DEFAULT 0,
                    imported_at TEXT NOT NULL
                );
                INSERT INTO sessions (
                    path, filename, media_kind, analysis_kind, status, duration_sec, imported_at
                ) VALUES (
                    '/tmp/legacy.mp4', 'legacy.mp4', 'flight_capture', 'legacy_import',
                    'ready', 3.5, '2026-04-01T00:00:00Z'
                );
                "#,
            )
            .expect("seed legacy database");
    }

    let store = SessionStore::open(&path).expect("open legacy database");
    let sessions = store.list_sessions().expect("legacy session survives");
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].filename, "legacy.mp4");

    for table in [
        "abstraction_epochs",
        "abstraction_spaces",
        "analysis_jobs",
        "graph_fragments",
        "world_regions",
        "plan_runs",
        "planner_snapshots",
        "trajectory_candidates",
        "mission_portfolios",
        "mission_instances",
        "mission_phases",
        "outcome_assessments",
        "sync_ops",
        "world_model_proposals",
    ] {
        let exists: i64 = store
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                [table],
                |row| row.get(0),
            )
            .expect("table lookup");
        assert_eq!(exists, 1, "expected migrated table {table}");
    }

    let version: i64 = store
        .connection()
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("version");
    assert_eq!(version, 1);
}

/// A session with one completed analysis job: the least a stored fragment needs.
struct Seeded {
    session_id: i64,
    job_id: i64,
}

fn seed_session(store: &SessionStore, path: &str) -> Seeded {
    let session_id = store.upsert_session(&session(path)).expect("session");
    let job_id = store
        .upsert_analysis_job(&AnalysisJobUpsert {
            session_id,
            environment_id: Some(1),
            job_kind: AnalysisJobKind::GraphInference,
            status: AnalysisJobStatus::Complete,
            requested_at: "2026-09-11T01:00:00Z".to_string(),
            started_at: Some("2026-09-11T01:00:01Z".to_string()),
            completed_at: Some("2026-09-11T01:00:02Z".to_string()),
            window_start_sec: Some(0.0),
            window_end_sec: Some(1.0),
            spec_json: "{}".to_string(),
            summary_json: "{}".to_string(),
            failure_json: None,
        })
        .expect("analysis job");
    Seeded { session_id, job_id }
}

/// Stores one region backed by a fragment whose inference was exact, which is
/// what makes the planner willing to route through it.
#[allow(clippy::too_many_arguments)]
fn exact_region(
    store: &SessionStore,
    seeded: &Seeded,
    environment_id: i64,
    region_key: &str,
    centroid_json: &str,
    confidence: f64,
    region_kind: RegionKind,
    signature_hash: &str,
    support_point_count: i64,
) -> i64 {
    let fragment_id = store
        .upsert_graph_fragment(&GraphFragmentUpsert {
            analysis_job_id: seeded.job_id,
            session_id: seeded.session_id,
            epoch_id: None,
            stream_key: "camera.front".to_string(),
            fragment_key: format!("fragment-{region_key}"),
            graph_kind: GraphKind::LocalSpace,
            graph_form: GraphForm::Tree,
            exactness: ExactnessKind::Exact,
            variable_count: 3,
            factor_count: 2,
            tree_width: Some(2),
            root_variable_key: Some("x0".to_string()),
            window_start_sec: 0.0,
            window_end_sec: 1.0,
            summary_json: "{}".to_string(),
        })
        .expect("graph fragment");

    store
        .upsert_world_region(&WorldRegionUpsert {
            environment_id: Some(environment_id),
            session_id: seeded.session_id,
            source_fragment_id: fragment_id,
            region_key: region_key.to_string(),
            region_kind,
            support_point_count,
            confidence,
            centroid_json: centroid_json.to_string(),
            bounds_json: "{}".to_string(),
            signature_hash: signature_hash.to_string(),
            metadata_json: "{}".to_string(),
        })
        .expect("world region")
}

fn plan_request(
    environment_id: i64,
    source_region_id: i64,
    target_region_id: i64,
    started_at: &str,
) -> DeliberatePlanRequest {
    DeliberatePlanRequest {
        environment_id,
        planner_kind: "deliberate".to_string(),
        started_at: started_at.to_string(),
        completed_at: Some("2026-09-11T04:00:00Z".to_string()),
        source_region_id,
        target_region_id,
        thresholds: DeliberatePlanThresholds::default(),
    }
}

#[test]
fn deliberate_plan_walks_exact_regions_and_records_each_step() {
    let dir = TempDir::new().expect("temp dir");
    let path = database_path(&dir);
    let store = SessionStore::open(&path).expect("open store");
    let seeded = seed_session(&store, "/tmp/plan.mp4");

    let start = exact_region(&store, &seeded, 7, "start", "[0.0,0.0,0.0]", 0.92, RegionKind::Corridor, "sig-start", 40);
    let mid = exact_region(&store, &seeded, 7, "mid", "[8.0,0.0,0.0]", 0.80, RegionKind::Corridor, "sig-mid", 30);
    let goal = exact_region(&store, &seeded, 7, "goal", "[30.0,0.0,0.0]", 0.90, RegionKind::Corridor, "sig-goal", 35);
    assert!(exact_region(&store, &seeded, 7, "rock", "[8.0,6.0,0.0]", 0.30, RegionKind::ObstacleCluster, "sig-rock", 10) > 0);

    // The direct hop is 30 ft, past the 25 ft edge cap, so the route has to be
    // walked through the midpoint.
    let run = store
        .generate_deliberate_plan(&plan_request(7, start, goal, "2026-09-11T02:00:00Z"))
        .expect("plan");

    assert_eq!(run.status, PlanStatus::Complete);
    assert_eq!(run.source_region_id, Some(start));
    assert_eq!(run.target_region_id, Some(goal));
    assert!(run.path_cost > 0.0, "route cost should be positive");
    // The riskliest hop is mid -> goal, at 0.2125.
    assert!((run.risk_score - 0.2125).abs() < 1e-9, "{}", run.risk_score);
    // The obstacle sits 6 ft from the midpoint, which caps the route clearance.
    assert!((run.clearance_min_ft - 6.0).abs() < 1e-9, "{}", run.clearance_min_ft);

    let steps = store.list_plan_run_regions(run.id).expect("steps");
    assert_eq!(
        steps.iter().map(|step| step.region_id).collect::<Vec<_>>(),
        vec![start, mid, goal]
    );
    assert_eq!(
        steps.iter().map(|step| step.step_index).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    assert!(steps.windows(2).all(|pair| pair[0].cumulative_cost < pair[1].cumulative_cost));
    assert!(steps.windows(2).all(|pair| pair[0].cumulative_risk <= pair[1].cumulative_risk));

    let summary: serde_json::Value = serde_json::from_str(&run.summary_json).expect("summary json");
    assert_eq!(summary["path_region_ids"], serde_json::json!([start, mid, goal]));
}

#[test]
fn deliberate_plan_reports_why_no_route_exists() {
    let dir = TempDir::new().expect("temp dir");
    let path = database_path(&dir);
    let store = SessionStore::open(&path).expect("open store");
    let seeded = seed_session(&store, "/tmp/no-path.mp4");

    let start = exact_region(&store, &seeded, 7, "start", "[0.0,0.0,0.0]", 0.92, RegionKind::Corridor, "sig-start", 40);
    let faint = exact_region(&store, &seeded, 7, "faint", "[4.0,0.0,0.0]", 0.40, RegionKind::Corridor, "sig-faint", 20);
    let far = exact_region(&store, &seeded, 7, "far", "[400.0,0.0,0.0]", 0.95, RegionKind::Corridor, "sig-far", 20);

    // A target under the free-space confidence floor is refused before the
    // frontier is expanded.
    let gated = store
        .generate_deliberate_plan(&plan_request(7, start, faint, "2026-09-11T03:00:00Z"))
        .expect("gated plan");
    assert_eq!(gated.status, PlanStatus::NoFeasiblePath);
    let summary: serde_json::Value = serde_json::from_str(&gated.summary_json).expect("summary");
    assert_eq!(summary["reason"], "confidence_or_exactness_gate_failed");
    assert!(store.list_plan_run_regions(gated.id).expect("steps").is_empty());

    // A target past every edge's distance cap is simply unreachable.
    let unreachable = store
        .generate_deliberate_plan(&plan_request(7, start, far, "2026-09-11T03:01:00Z"))
        .expect("unreachable plan");
    assert_eq!(unreachable.status, PlanStatus::NoFeasiblePath);
    let summary: serde_json::Value = serde_json::from_str(&unreachable.summary_json).expect("summary");
    assert_eq!(summary["reason"], "no_connected_path");

    assert_eq!(store.list_plan_runs(7).expect("runs").len(), 2);
}

#[test]
fn merge_candidate_generation_keeps_only_pairs_over_the_thresholds() {
    let dir = TempDir::new().expect("temp dir");
    let path = database_path(&dir);
    let store = SessionStore::open(&path).expect("open store");
    let left = seed_session(&store, "/tmp/merge-left.mp4");
    let right = seed_session(&store, "/tmp/merge-right.mp4");

    let left_corridor = exact_region(&store, &left, 9, "left-corridor", "[0.0,0.0,0.0]", 0.90, RegionKind::Corridor, "shared-signature", 40);
    let left_field = exact_region(&store, &left, 9, "left-field", "[50.0,0.0,0.0]", 0.50, RegionKind::LandmarkField, "left-only", 10);
    let right_corridor = exact_region(&store, &right, 9, "right-corridor", "[1.0,0.0,0.0]", 0.88, RegionKind::Corridor, "shared-signature", 38);
    let right_other = exact_region(&store, &right, 9, "right-other", "[50.0,0.0,0.0]", 0.50, RegionKind::ObstacleCluster, "right-only", 10);
    assert!(left_field > 0 && right_other > 0);

    let report = store
        .generate_session_merge_candidates(
            left.session_id,
            right.session_id,
            &MergeCandidateThresholds::default(),
        )
        .expect("generation");

    assert_eq!(report.evaluated_pairs, 4, "every left/right region pair is scored");
    assert_eq!(report.persisted_candidates, 1, "only the matching corridor pair clears the thresholds");
    assert_eq!(report.persisted_region_links, 1);

    let candidates = store
        .list_session_merge_candidates(left.session_id, right.session_id)
        .expect("candidates");
    assert_eq!(candidates.len(), 1);
    let kept = &candidates[0];
    assert_eq!(kept.left_region_id, left_corridor);
    assert_eq!(kept.right_region_id, right_corridor);
    assert_eq!(kept.candidate_kind, LinkKind::Overlap);
    assert_eq!(kept.state, CandidateState::Proposed);
    assert!((kept.score - 0.9725).abs() < 1e-9, "{}", kept.score);
    assert!((kept.transform_consistency - 0.945).abs() < 1e-9, "{}", kept.transform_consistency);
    assert!((kept.contradiction_score - 0.02525).abs() < 1e-9, "{}", kept.contradiction_score);

    let reasons: serde_json::Value = serde_json::from_str(&kept.reason_json).expect("reason json");
    assert_eq!(reasons["factors"]["signature_score"], 1.0);
    assert_eq!(reasons["thresholds"]["overlap_min"], 0.55);

    assert_eq!(
        store
            .list_session_merge_candidates_for_session(left.session_id)
            .expect("for session")
            .len(),
        1
    );

    let links = store.list_region_links(left_corridor).expect("links");
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].right_region_id, right_corridor);
    assert_eq!(links[0].state, CandidateState::Proposed);
    let transform: serde_json::Value =
        serde_json::from_str(&links[0].relative_transform_json).expect("transform json");
    assert!(transform["translation"].is_object(), "{transform}");
    assert!(store.list_region_links(left_field).expect("rejected pair").is_empty());

    // A stricter pass writes nothing new and leaves the earlier candidates alone.
    let stricter = MergeCandidateThresholds {
        overlap_min: 0.99,
        transform_consistency_min: 0.99,
        contradiction_max: 0.01,
    };
    let report = store
        .generate_session_merge_candidates(left.session_id, right.session_id, &stricter)
        .expect("strict generation");
    assert_eq!(report.persisted_candidates, 0);
    assert_eq!(report.persisted_region_links, 0);
    assert_eq!(
        store
            .list_session_merge_candidates(left.session_id, right.session_id)
            .expect("unchanged")
            .len(),
        1
    );
}
