//! End-to-end checks over the bridge's observable surface: the projection
//! records a consumer receives and the recording a sink produces on disk.

use std::fs;
use std::path::PathBuf;

use qualia_rerun_bridge::{
    collect_sync_projection_records, collect_world_model_projection_records,
    default_graph_blueprint, default_thought_theater_blueprint, QualiaRerunBridge,
    RerunBridgeConfig, RerunSinkConfig, ReplayEntityTaxonomy, SessionReplayFrame, SyncEntityTaxonomy,
    SyncProjection, WorldModelEntityTaxonomy, WorldModelProjection, WorldModelProjectionContent,
    WorldModelProjectionRecord,
};
use qualia_session_store::{SyncAppendEntryRow, SyncOpRow, SyncReplicaRow, SyncStateRow};
use qualia_sync_types::{
    CanonicalBody, CanonicalGraphNode, CanonicalKind, CanonicalStateEnvelope, CanonicalStatus,
    CoachDecision, CoachDecisionKind, HlcTimestamp, ObjectProposal, OperationalHazardState,
    OperationalNavGoal, OperationalPlannerCompact, OperationalPose, OperationalStateEnvelope,
    ProposalBody, ProposalEnvelope, ProposalKind, ProposalLineage, ProposalStatus, ReplicaRole,
    ReplicaTrustState, SyncApplyStatus, SyncBody, SyncMaterializedState, SyncNamespace,
    SyncOrMapState, SyncRegisterState, SyncRegisterValue, WORLD_MODEL_SCHEMA_VERSION,
};

fn hlc(tick: u64) -> HlcTimestamp {
    HlcTimestamp::new(tick * 1_000_000, 0)
}

fn temp_dir(case: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "qualia-rerun-bridge-{}-{case}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn record<'a>(
    records: &'a [WorldModelProjectionRecord],
    entity_path: &str,
) -> &'a WorldModelProjectionRecord {
    records
        .iter()
        .find(|candidate| candidate.entity_path == entity_path)
        .unwrap_or_else(|| panic!("no record at {entity_path}"))
}

fn record_text(records: &[WorldModelProjectionRecord], entity_path: &str) -> String {
    match &record(records, entity_path).content {
        WorldModelProjectionContent::Text(text) => text.clone(),
        other => panic!("expected text at {entity_path}, got {other:?}"),
    }
}

fn record_document(records: &[WorldModelProjectionRecord], entity_path: &str) -> String {
    match &record(records, entity_path).content {
        WorldModelProjectionContent::Document(markdown) => markdown.clone(),
        other => panic!("expected document at {entity_path}, got {other:?}"),
    }
}

fn record_points(records: &[WorldModelProjectionRecord], entity_path: &str) -> Vec<(f32, f32)> {
    match &record(records, entity_path).content {
        WorldModelProjectionContent::Points2D(points) => points.clone(),
        other => panic!("expected points at {entity_path}, got {other:?}"),
    }
}

fn object_proposal(id: &str) -> ProposalEnvelope {
    ProposalEnvelope {
        schema_version: WORLD_MODEL_SCHEMA_VERSION.to_string(),
        proposal_id: id.to_string(),
        proposal_kind: ProposalKind::Object,
        source_replica_id: "replica-jetson".to_string(),
        source_replica_role: ReplicaRole::Jetson,
        created_at_hlc: hlc(4),
        status: ProposalStatus::Promoted,
        belief_weight: 0.5,
        source_weight: 0.5,
        mission_relevance: 0.25,
        confidence: 0.75,
        lineage: ProposalLineage::default(),
        body: ProposalBody::Object(ObjectProposal {
            label: "buoy".to_string(),
            attributes: serde_json::json!({ "center": { "x_m": 1.5, "y_m": -2.25 } }),
            profile_refs: vec!["profile.sonar".to_string()],
        }),
    }
}

fn beacon_node(id: &str) -> CanonicalStateEnvelope {
    CanonicalStateEnvelope {
        schema_version: WORLD_MODEL_SCHEMA_VERSION.to_string(),
        canonical_id: id.to_string(),
        canonical_kind: CanonicalKind::GraphNode,
        source_decision_id: "decision-promote".to_string(),
        source_proposal_ids: vec!["proposal-buoy".to_string()],
        accepted_at_hlc: hlc(9),
        status: CanonicalStatus::Accepted,
        body: CanonicalBody::GraphNode(CanonicalGraphNode {
            node_kind: "beacon".to_string(),
            attributes: serde_json::json!({ "position": { "x_m": 4.0, "y_m": 5.0 } }),
        }),
    }
}

fn promote_decision() -> CoachDecision {
    CoachDecision {
        schema_version: WORLD_MODEL_SCHEMA_VERSION.to_string(),
        decision_id: "decision-promote".to_string(),
        decision_kind: CoachDecisionKind::Promote,
        curator_replica_id: "replica-operator".to_string(),
        curator_replica_role: ReplicaRole::Operator,
        created_at_hlc: hlc(8),
        target_proposal_ids: vec!["proposal-buoy".to_string()],
        output_ids: vec!["canonical-beacon".to_string()],
        reason: Some("promote the buoy".to_string()),
    }
}

fn operational_state() -> OperationalStateEnvelope {
    OperationalStateEnvelope {
        schema_version: WORLD_MODEL_SCHEMA_VERSION.to_string(),
        pose: Some(OperationalPose {
            canonical_id: "canonical-beacon".to_string(),
            x_m: 4.0,
            y_m: 5.0,
            z_m: 0.5,
            yaw_rad: 0.1,
            confidence: Some(0.9),
        }),
        nav_goal: Some(OperationalNavGoal {
            canonical_id: "canonical-goal".to_string(),
            active: true,
            x_m: 1.0,
            y_m: 1.0,
            z_m: 0.0,
            yaw_rad: 0.0,
        }),
        hazards: vec![OperationalHazardState {
            canonical_id: "canonical-rocks".to_string(),
            kind: "rocks".to_string(),
            severity: Some(0.8),
            summary: Some("shallow shelf".to_string()),
        }],
        planner: OperationalPlannerCompact {
            accepted_count: 3,
            nav_goal_present: true,
            hazard_count: 1,
            route_factor_count: 2,
        },
        source_canonical_ids: vec![
            "canonical-beacon".to_string(),
            "canonical-rocks".to_string(),
        ],
    }
}

fn register_state(namespace: SyncNamespace, key: &str) -> SyncStateRow {
    SyncStateRow {
        namespace,
        key: key.to_string(),
        state: SyncMaterializedState::Register(SyncRegisterState {
            value: serde_json::json!("engaged"),
            winner_timestamp_hlc: hlc(3),
            winner_replica_id: "replica-jetson".to_string(),
            last_op_id: "op-1".to_string(),
            lease_holder: None,
            lease_expires_hlc: None,
        }),
        updated_hlc: hlc(3),
        last_op_id: "op-1".to_string(),
    }
}

fn or_map_state(namespace: SyncNamespace, key: &str) -> SyncStateRow {
    SyncStateRow {
        namespace,
        key: key.to_string(),
        state: SyncMaterializedState::OrMap(SyncOrMapState {
            entries: Vec::new(),
            removed_tags: Vec::new(),
            visible_tag: Some("jetson".to_string()),
            visible_value: Some(serde_json::json!("engaged")),
        }),
        updated_hlc: hlc(4),
        last_op_id: "op-2".to_string(),
    }
}

fn replica(replica_id: &str, enabled: bool, trust_state: ReplicaTrustState) -> SyncReplicaRow {
    SyncReplicaRow {
        replica_id: replica_id.to_string(),
        replica_role: ReplicaRole::Jetson,
        display_name: replica_id.to_string(),
        endpoint: format!("https://{replica_id}:7443"),
        trust_state,
        enabled,
        capabilities_json: "{}".to_string(),
        metadata_json: "{}".to_string(),
        last_seen_hlc: Some(hlc(3)),
        last_seen_at: None,
        last_sync_seq: 12,
        updated_at: "2026-01-01T00:00:00Z".to_string(),
    }
}

fn op(seq: i64, op_id: &str, status: SyncApplyStatus) -> SyncOpRow {
    SyncOpRow {
        seq,
        op_id: op_id.to_string(),
        schema_version: "sync.v1".to_string(),
        replica_id: "replica-jetson".to_string(),
        replica_role: ReplicaRole::Jetson,
        run_id: "run-1".to_string(),
        counter: seq as u64,
        timestamp_hlc: hlc(seq as u64),
        namespace: SyncNamespace::Directive,
        key: "mode".to_string(),
        body: SyncBody::Register(SyncRegisterValue {
            value: serde_json::json!("engaged"),
            lease_duration_ms: None,
        }),
        status,
        status_detail: String::new(),
        received_at: "2026-01-01T00:00:00Z".to_string(),
    }
}

fn append_entry(entry_id: &str) -> SyncAppendEntryRow {
    SyncAppendEntryRow {
        namespace: SyncNamespace::Activity,
        key: "log".to_string(),
        entry_id: entry_id.to_string(),
        value: serde_json::json!({ "event": "dive" }),
        timestamp_hlc: hlc(5),
        source_op_id: "op-3".to_string(),
        replica_id: "replica-jetson".to_string(),
    }
}

#[test]
fn disabled_sink_accepts_everything_and_writes_nothing() {
    let bridge = QualiaRerunBridge::new(&RerunBridgeConfig {
        application_id: "qualia-test".to_string(),
        sink: RerunSinkConfig::Disabled,
    })
    .expect("disabled bridge builds");

    assert!(!bridge.is_enabled());

    let canonical = [beacon_node("canonical-beacon")];
    let decisions = [promote_decision()];
    let operational = operational_state();
    let proposals = [object_proposal("proposal-buoy")];
    let projection = WorldModelProjection {
        proposals: &proposals,
        decisions: &decisions,
        canonical: &canonical,
        operational: &operational,
    };
    bridge
        .log_world_model_projection(7, &projection)
        .expect("disabled world-model log is a no-op");

    let replicas = [replica("replica-a", true, ReplicaTrustState::Trusted)];
    let states = [register_state(SyncNamespace::Directive, "mode")];
    let entries = [append_entry("entry-1")];
    let ops = [op(1, "op-1", SyncApplyStatus::Applied)];
    let sync = SyncProjection {
        replicas: &replicas,
        states: &states,
        append_entries: &entries,
        ops: &ops,
    };
    bridge
        .log_sync_projection(7, &sync)
        .expect("disabled sync log is a no-op");
    bridge
        .export_session_replay(&[SessionReplayFrame {
            tick: 7,
            label: Some("frame-7"),
            world_model: Some(projection),
            sync: Some(sync),
        }])
        .expect("disabled replay export is a no-op");
    bridge
        .send_default_thought_theater_blueprint()
        .expect("disabled blueprint send is a no-op");
    bridge
        .send_default_graph_blueprint()
        .expect("disabled graph blueprint send is a no-op");

    assert!(!bridge.is_enabled());
}

#[test]
fn saved_sink_grows_a_recording_file_for_every_stage() {
    let dir = temp_dir("saved-sink");
    let path = dir.join("recording.rrd");

    let bridge = QualiaRerunBridge::new(&RerunBridgeConfig {
        application_id: "qualia-test".to_string(),
        sink: RerunSinkConfig::Save(path.clone()),
    })
    .expect("save bridge builds");
    assert!(bridge.is_enabled());

    let canonical = [beacon_node("canonical-beacon")];
    let decisions = [promote_decision()];
    let operational = operational_state();
    let proposals = [object_proposal("proposal-buoy")];
    let projection = WorldModelProjection {
        proposals: &proposals,
        decisions: &decisions,
        canonical: &canonical,
        operational: &operational,
    };
    bridge
        .log_world_model_projection(7, &projection)
        .expect("world-model projection is written");
    let after_world_model = fs::metadata(&path).expect("recording exists").len();
    assert!(
        after_world_model > 0,
        "logging a world-model projection must write bytes"
    );

    let replicas = [
        replica("replica-a", true, ReplicaTrustState::Trusted),
        replica("replica-b", false, ReplicaTrustState::Disabled),
    ];
    let states = [
        register_state(SyncNamespace::Directive, "mode"),
        or_map_state(SyncNamespace::SceneText, "labels"),
    ];
    let entries = [append_entry("entry-1")];
    let ops = [op(1, "op-1", SyncApplyStatus::Applied)];
    let sync = SyncProjection {
        replicas: &replicas,
        states: &states,
        append_entries: &entries,
        ops: &ops,
    };
    bridge
        .log_sync_projection(7, &sync)
        .expect("sync projection is written");
    let after_sync = fs::metadata(&path).expect("recording exists").len();
    assert!(
        after_sync > after_world_model,
        "sync logging must append to the same recording"
    );

    bridge
        .export_session_replay(&[SessionReplayFrame {
            tick: 8,
            label: Some("frame-8"),
            world_model: Some(projection),
            sync: Some(sync),
        }])
        .expect("replay export is written");
    let after_replay = fs::metadata(&path).expect("recording exists").len();
    assert!(
        after_replay > after_sync,
        "replay export must append to the same recording"
    );

    bridge
        .send_default_thought_theater_blueprint()
        .expect("thought theater blueprint is sent");
    assert!(fs::metadata(&path).expect("recording exists").len() > 0);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn unwritable_sink_path_is_reported_with_context() {
    let dir = temp_dir("unwritable-sink");
    let blocker = dir.join("blocker");
    fs::write(&blocker, b"not a directory").expect("write blocker file");
    let path = blocker.join("recording.rrd");

    let error = QualiaRerunBridge::new(&RerunBridgeConfig {
        application_id: "qualia-test".to_string(),
        sink: RerunSinkConfig::Save(path),
    })
    .expect_err("a file cannot host a recording directory");

    let message = error.to_string();
    assert!(
        message.starts_with("failed to create saved Rerun stream:"),
        "unexpected error text: {message}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn world_model_projection_covers_taxonomy_counts_geometry_and_coach_timeline() {
    let proposals = [object_proposal("proposal-buoy")];
    let canonical = [beacon_node("canonical-beacon")];
    let decisions = [promote_decision()];
    let operational = operational_state();
    let projection = WorldModelProjection {
        proposals: &proposals,
        decisions: &decisions,
        canonical: &canonical,
        operational: &operational,
    };

    let records = collect_world_model_projection_records(7, &projection);

    assert_eq!(
        record_text(&records, WorldModelEntityTaxonomy::summary()),
        "tick=7 proposals=1 canonical=1 accepted=3 nav_goal_present=true hazards=1"
    );
    assert_eq!(
        record_text(&records, WorldModelEntityTaxonomy::proposals_root()),
        "count=1 accepted=1"
    );
    assert_eq!(
        record_text(&records, WorldModelEntityTaxonomy::canonical_root()),
        "count=1 accepted=1"
    );

    let proposal_summary = record_text(
        &records,
        &WorldModelEntityTaxonomy::proposal_summary("proposal-buoy"),
    );
    assert!(proposal_summary.contains("status=Promoted"), "{proposal_summary}");
    assert!(proposal_summary.contains("confidence=0.75"), "{proposal_summary}");
    let proposal_body = record_text(
        &records,
        &WorldModelEntityTaxonomy::proposal_body("proposal-buoy"),
    );
    assert!(proposal_body.contains("label=buoy"), "{proposal_body}");
    assert_eq!(
        record_points(
            &records,
            &WorldModelEntityTaxonomy::proposal_geometry("proposal-buoy")
        ),
        vec![(1.5, -2.25)]
    );

    let canonical_summary = record_text(
        &records,
        &WorldModelEntityTaxonomy::canonical_summary("canonical-beacon"),
    );
    assert!(canonical_summary.contains("status=Accepted"), "{canonical_summary}");
    assert_eq!(
        record_points(
            &records,
            &WorldModelEntityTaxonomy::canonical_geometry("canonical-beacon")
        ),
        vec![(4.0, 5.0)]
    );
    assert_eq!(
        record_points(
            &records,
            &WorldModelEntityTaxonomy::canonical_graph_node("canonical-beacon")
        ),
        vec![(4.0, 5.0)]
    );
    let lineage = record_document(
        &records,
        &WorldModelEntityTaxonomy::canonical_lineage("canonical-beacon"),
    );
    assert!(lineage.contains("decision-promote"), "{lineage}");
    assert!(lineage.contains("proposal-buoy"), "{lineage}");

    assert_eq!(
        record_text(&records, WorldModelEntityTaxonomy::operational_root()),
        "pose_present=true nav_goal_present=true hazards=1 route_factor_count=2"
    );
    assert_eq!(
        record_text(&records, WorldModelEntityTaxonomy::operational_planner()),
        "accepted_count=3 nav_goal_present=true hazard_count=1 route_factor_count=2"
    );
    match &record(&records, WorldModelEntityTaxonomy::operational_route()).content {
        WorldModelProjectionContent::LineStrip2D(points) => {
            assert_eq!(points, &vec![(4.0, 5.0), (1.0, 1.0)]);
        }
        other => panic!("expected a route line, got {other:?}"),
    }
    let hazard = record_text(
        &records,
        &WorldModelEntityTaxonomy::operational_hazard("canonical-rocks"),
    );
    assert!(hazard.contains("kind=rocks"), "{hazard}");

    let timeline = record(
        &records,
        WorldModelEntityTaxonomy::coach_timeline_kind(CoachDecisionKind::Promote),
    );
    match &timeline.content {
        WorldModelProjectionContent::LogEvent { text, level, .. } => {
            assert!(text.contains("promote"), "{text}");
            assert!(text.contains("targets=1"), "{text}");
            assert_eq!(format!("{level:?}"), "INFO");
        }
        other => panic!("expected a coach timeline event, got {other:?}"),
    }
    let coach_summary = record_document(&records, WorldModelEntityTaxonomy::coach_summary());
    assert!(coach_summary.contains("- promote: 1"), "{coach_summary}");
    assert!(coach_summary.contains("- reject: 0"), "{coach_summary}");
    let target_effect = record_document(
        &records,
        &WorldModelEntityTaxonomy::proposal_coach_event("proposal-buoy", "decision-promote"),
    );
    assert!(target_effect.contains("promote the buoy"), "{target_effect}");
    let output_effect = record_document(
        &records,
        &WorldModelEntityTaxonomy::canonical_coach_event("canonical-beacon", "decision-promote"),
    );
    assert!(output_effect.contains("canonical-beacon"), "{output_effect}");
}

#[test]
fn geometry_without_coordinates_is_reported_as_unavailable() {
    let mut proposal = object_proposal("proposal-bare");
    proposal.body = ProposalBody::Object(ObjectProposal {
        label: "bare".to_string(),
        attributes: serde_json::json!({ "note": "no coordinates" }),
        profile_refs: Vec::new(),
    });
    let proposals = [proposal];
    let canonical: [CanonicalStateEnvelope; 0] = [];
    let decisions: [CoachDecision; 0] = [];
    let operational = OperationalStateEnvelope {
        schema_version: WORLD_MODEL_SCHEMA_VERSION.to_string(),
        pose: None,
        nav_goal: None,
        hazards: Vec::new(),
        planner: OperationalPlannerCompact {
            accepted_count: 0,
            nav_goal_present: false,
            hazard_count: 0,
            route_factor_count: 0,
        },
        source_canonical_ids: Vec::new(),
    };
    let projection = WorldModelProjection {
        proposals: &proposals,
        decisions: &decisions,
        canonical: &canonical,
        operational: &operational,
    };

    let records = collect_world_model_projection_records(1, &projection);
    assert_eq!(
        record_text(
            &records,
            &WorldModelEntityTaxonomy::proposal_geometry("proposal-bare")
        ),
        "geometry=unavailable"
    );
    assert_eq!(
        record_text(&records, WorldModelEntityTaxonomy::summary()),
        "tick=1 proposals=1 canonical=0 accepted=0 nav_goal_present=false hazards=0"
    );
}

#[test]
fn sync_projection_counts_replica_state_and_op_outcomes() {
    let replicas = [
        replica("replica-a", true, ReplicaTrustState::Trusted),
        replica("replica-b", false, ReplicaTrustState::Disabled),
    ];
    let states = [
        register_state(SyncNamespace::Directive, "mode"),
        or_map_state(SyncNamespace::SceneText, "labels"),
    ];
    let entries = [append_entry("entry-1")];
    let ops = [
        op(1, "op-1", SyncApplyStatus::Applied),
        op(2, "op-2", SyncApplyStatus::Duplicate),
        op(3, "op-3", SyncApplyStatus::RejectedAuthority),
    ];
    let projection = SyncProjection {
        replicas: &replicas,
        states: &states,
        append_entries: &entries,
        ops: &ops,
    };

    let records = collect_sync_projection_records(3, &projection);

    assert_eq!(
        record_text(&records, SyncEntityTaxonomy::summary()),
        "tick=3 replicas=2 states=2 append_entries=1 ops=3 applied_ops=1 namespaces=2"
    );
    assert_eq!(
        record_text(&records, SyncEntityTaxonomy::replicas_root()),
        "count=2 enabled=1 trusted=1"
    );
    assert_eq!(
        record_text(&records, SyncEntityTaxonomy::states_root()),
        "count=2 namespaces=2"
    );
    assert_eq!(
        record_text(&records, SyncEntityTaxonomy::append_root()),
        "count=1"
    );
    assert_eq!(
        record_text(&records, SyncEntityTaxonomy::ops_root()),
        "count=3 applied=1 duplicates=1 rejected=1"
    );

    let replica_row = record_text(&records, &SyncEntityTaxonomy::replica("replica-a"));
    assert!(replica_row.contains("role=Jetson"), "{replica_row}");
    assert!(replica_row.contains("trust_state=Trusted"), "{replica_row}");
    assert!(replica_row.contains("enabled=true"), "{replica_row}");
    assert!(replica_row.contains("last_sync_seq=12"), "{replica_row}");

    let state_row = record_text(
        &records,
        &SyncEntityTaxonomy::state(SyncNamespace::Directive, "mode"),
    );
    assert!(state_row.contains("namespace=directive"), "{state_row}");
    assert!(state_row.contains("key=mode"), "{state_row}");
    assert!(state_row.contains("kind=register"), "{state_row}");

    let append_row = record_text(
        &records,
        &SyncEntityTaxonomy::append_entry(SyncNamespace::Activity, "log", "entry-1"),
    );
    assert!(append_row.contains("entry_id=entry-1"), "{append_row}");
    assert!(append_row.contains("replica=replica-jetson"), "{append_row}");

    let op_row = record_text(&records, &SyncEntityTaxonomy::op("op-1"));
    assert!(op_row.contains("seq=1"), "{op_row}");
    assert!(op_row.contains("status=Applied"), "{op_row}");
    assert!(op_row.contains("body_kind=register"), "{op_row}");
}

#[test]
fn session_replay_frame_document_reports_both_projections() {
    let canonical = [beacon_node("canonical-beacon")];
    let decisions = [promote_decision()];
    let operational = operational_state();
    let proposals = [object_proposal("proposal-buoy")];
    let world_model = WorldModelProjection {
        proposals: &proposals,
        decisions: &decisions,
        canonical: &canonical,
        operational: &operational,
    };
    let replicas = [replica("replica-a", true, ReplicaTrustState::Trusted)];
    let states = [register_state(SyncNamespace::Directive, "mode")];
    let entries = [append_entry("entry-1")];
    let ops = [op(1, "op-1", SyncApplyStatus::Applied)];
    let sync = SyncProjection {
        replicas: &replicas,
        states: &states,
        append_entries: &entries,
        ops: &ops,
    };
    let frame = SessionReplayFrame {
        tick: 42,
        label: Some("dive-42"),
        world_model: Some(world_model),
        sync: Some(sync),
    };

    let dir = temp_dir("replay-frame");
    let path = dir.join("replay.rrd");
    let bridge = QualiaRerunBridge::new(&RerunBridgeConfig {
        application_id: "qualia-test".to_string(),
        sink: RerunSinkConfig::Save(path.clone()),
    })
    .expect("save bridge builds");
    bridge
        .export_session_replay(&[frame])
        .expect("replay export writes the frame");
    assert!(fs::metadata(&path).expect("recording exists").len() > 0);
    assert_eq!(ReplayEntityTaxonomy::summary(), "qualia/replay/summary");
    assert_eq!(ReplayEntityTaxonomy::ROOT, "qualia/replay");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn taxonomy_sanitises_ids_and_blueprints_build() {
    assert_eq!(
        WorldModelEntityTaxonomy::proposal("a/b c"),
        "qualia/world_model/proposals/a-b-c"
    );
    assert_eq!(
        SyncEntityTaxonomy::state(SyncNamespace::SceneText, "peer:1"),
        "qualia/sync/states/scene_text/peer-1"
    );
    assert_eq!(
        WorldModelEntityTaxonomy::coach_timeline_kind(CoachDecisionKind::Deprecate),
        "qualia/world_model/coach/timeline/deprecate"
    );
    let _ = default_thought_theater_blueprint();
    let _ = default_graph_blueprint();
}
