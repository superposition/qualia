//! Behavioural tests for the persistent session store, run against real
//! temporary SQLite databases (never `:memory:`) so that reopen and on-disk
//! schema behaviour are exercised.

use qualia_session_store::mission_types::{AnalysisJobKind, AnalysisJobStatus};
use qualia_session_store::{
    AbstractStateSample, AbstractionEpochUpsert, AbstractionSpaceUpsert, AnalysisJobUpsert,
    MissionInstanceUpsert, MissionPhaseUpsert, MissionPortfolioUpsert, OutcomeAssessmentUpsert,
    SessionStore, SessionUpsert,
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
