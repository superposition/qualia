//! Row mappers and small geometry helpers shared by the store's method groups.
//!
//! These are crate-internal: the public surface is `SessionStore` plus the row
//! and upsert types.

use crate::mission_types::{
    AnalysisJobRow, BeliefMatrixRow, GraphFragmentRow, MessageSnapshotRow, PlanRunRow,
    RegionLinkRow, SessionMergeCandidateRow, WorldRegionRow,
};
use crate::{AbstractStateSampleRow, PlannerSnapshotRow};
use rusqlite::types::Type;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(crate) fn map_state_sample_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<AbstractStateSampleRow> {
    Ok(AbstractStateSampleRow {
        epoch_id: row.get(0)?,
        space_id: row.get(1)?,
        step: row.get(2)?,
        timestamp_sec: row.get(3)?,
        symbol_key: row.get(4)?,
        payload_json: row.get(5)?,
        confidence: row.get(6)?,
        sample_hash: row.get(7)?,
    })
}

pub(crate) fn enum_to_db_text<T: Serialize>(value: &T) -> rusqlite::Result<String> {
    match serde_json::to_value(value)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?
    {
        Value::String(text) => Ok(text),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

pub(crate) fn enum_from_db_text<T: for<'de> Deserialize<'de>>(value: String) -> rusqlite::Result<T> {
    serde_json::from_value(Value::String(value)).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(0, Type::Text, Box::new(error))
    })
}

/// SQLite sort key for a hybrid-logical clock stored in its unpadded
/// `{physical_ns}:{logical}` text form.
///
/// `HlcTimestamp` writes both decimal components without zero padding, so
/// comparing the raw text digit-by-digit inverts clock order as soon as the
/// component widths differ (`"...:10"` sorts before `"...:9"`). Padding each
/// component to its widest decimal width restores agreement with the derived
/// `(physical_ns, logical)` order for every representable `u64`/`u32` value.
/// The result is an `ORDER BY` fragment of two ascending terms.
pub(crate) fn hlc_order_key(column: &str) -> String {
    let physical = format!("substr({column}, 1, instr({column}, ':') - 1)");
    let logical = format!("substr({column}, instr({column}, ':') + 1)");
    format!(
        "substr('00000000000000000000', 1, 20 - length({physical})) || {physical}, \
         substr('0000000000', 1, 10 - length({logical})) || {logical}"
    )
}

pub(crate) fn map_analysis_job_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AnalysisJobRow> {
    Ok(AnalysisJobRow {
        id: row.get(0)?,
        session_id: row.get(1)?,
        environment_id: row.get(2)?,
        job_kind: enum_from_db_text(row.get(3)?)?,
        status: enum_from_db_text(row.get(4)?)?,
        requested_at: row.get(5)?,
        started_at: row.get(6)?,
        completed_at: row.get(7)?,
        window_start_sec: row.get(8)?,
        window_end_sec: row.get(9)?,
        spec_json: row.get(10)?,
        summary_json: row.get(11)?,
        failure_json: row.get(12)?,
    })
}

pub(crate) fn map_graph_fragment_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<GraphFragmentRow> {
    Ok(GraphFragmentRow {
        id: row.get(0)?,
        analysis_job_id: row.get(1)?,
        session_id: row.get(2)?,
        epoch_id: row.get(3)?,
        stream_key: row.get(4)?,
        fragment_key: row.get(5)?,
        graph_kind: enum_from_db_text(row.get(6)?)?,
        graph_form: enum_from_db_text(row.get(7)?)?,
        exactness: enum_from_db_text(row.get(8)?)?,
        variable_count: row.get(9)?,
        factor_count: row.get(10)?,
        tree_width: row.get(11)?,
        root_variable_key: row.get(12)?,
        window_start_sec: row.get(13)?,
        window_end_sec: row.get(14)?,
        summary_json: row.get(15)?,
    })
}

pub(crate) fn map_belief_matrix_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<BeliefMatrixRow> {
    Ok(BeliefMatrixRow {
        id: row.get(0)?,
        fragment_id: row.get(1)?,
        variable_key: row.get(2)?,
        domain_kind: enum_from_db_text(row.get(3)?)?,
        normalization_error: row.get(4)?,
        entropy: row.get(5)?,
        max_state_key: row.get(6)?,
        values_json: row.get(7)?,
        matrix_hash: row.get(8)?,
    })
}

pub(crate) fn map_message_snapshot_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<MessageSnapshotRow> {
    Ok(MessageSnapshotRow {
        id: row.get(0)?,
        fragment_id: row.get(1)?,
        edge_key: row.get(2)?,
        direction: enum_from_db_text(row.get(3)?)?,
        iteration_index: row.get(4)?,
        source_node_key: row.get(5)?,
        target_node_key: row.get(6)?,
        values_json: row.get(7)?,
        residual_norm: row.get(8)?,
        message_hash: row.get(9)?,
    })
}

pub(crate) fn map_world_region_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorldRegionRow> {
    Ok(WorldRegionRow {
        id: row.get(0)?,
        environment_id: row.get(1)?,
        session_id: row.get(2)?,
        source_fragment_id: row.get(3)?,
        region_key: row.get(4)?,
        region_kind: enum_from_db_text(row.get(5)?)?,
        support_point_count: row.get(6)?,
        confidence: row.get(7)?,
        centroid_json: row.get(8)?,
        bounds_json: row.get(9)?,
        signature_hash: row.get(10)?,
        metadata_json: row.get(11)?,
    })
}

pub(crate) fn map_region_link_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RegionLinkRow> {
    Ok(RegionLinkRow {
        id: row.get(0)?,
        left_region_id: row.get(1)?,
        right_region_id: row.get(2)?,
        link_kind: enum_from_db_text(row.get(3)?)?,
        score: row.get(4)?,
        relative_transform_json: row.get(5)?,
        contradiction_score: row.get(6)?,
        state: enum_from_db_text(row.get(7)?)?,
        evidence_json: row.get(8)?,
    })
}

pub(crate) fn map_session_merge_candidate_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<SessionMergeCandidateRow> {
    Ok(SessionMergeCandidateRow {
        id: row.get(0)?,
        left_session_id: row.get(1)?,
        right_session_id: row.get(2)?,
        left_region_id: row.get(3)?,
        right_region_id: row.get(4)?,
        candidate_kind: enum_from_db_text(row.get(5)?)?,
        score: row.get(6)?,
        transform_consistency: row.get(7)?,
        contradiction_score: row.get(8)?,
        state: enum_from_db_text(row.get(9)?)?,
        reason_json: row.get(10)?,
    })
}

pub(crate) fn map_plan_run_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PlanRunRow> {
    Ok(PlanRunRow {
        id: row.get(0)?,
        environment_id: row.get(1)?,
        session_id: row.get(2)?,
        source_region_id: row.get(3)?,
        target_region_id: row.get(4)?,
        planner_kind: row.get(5)?,
        status: enum_from_db_text(row.get(6)?)?,
        path_cost: row.get(7)?,
        risk_score: row.get(8)?,
        clearance_min_ft: row.get(9)?,
        started_at: row.get(10)?,
        completed_at: row.get(11)?,
        summary_json: row.get(12)?,
    })
}

pub(crate) fn map_planner_snapshot_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<PlannerSnapshotRow> {
    Ok(PlannerSnapshotRow {
        id: row.get(0)?,
        plan_run_id: row.get(1)?,
        session_id: row.get(2)?,
        graph_fragment_id: row.get(3)?,
        snapshot_key: row.get(4)?,
        status: row.get(5)?,
        window_start_sec: row.get(6)?,
        window_end_sec: row.get(7)?,
        selected_candidate_key: row.get(8)?,
        trace_key: row.get(9)?,
        trace_status: row.get(10)?,
        summary_json: row.get(11)?,
    })
}

pub(crate) fn clamp01(value: f64) -> f64 {
    value.clamp(0.0, 1.0)
}

pub(crate) fn parse_centroid(json: &str) -> Option<[f64; 3]> {
    let parsed: Value = serde_json::from_str(json).ok()?;
    let coordinates = parsed.as_array()?;
    if coordinates.len() < 3 {
        return None;
    }
    Some([
        coordinates.first()?.as_f64()?,
        coordinates.get(1)?.as_f64()?,
        coordinates.get(2)?.as_f64()?,
    ])
}

pub(crate) fn centroid_distance_score(left_centroid_json: &str, right_centroid_json: &str) -> f64 {
    match (
        parse_centroid(left_centroid_json),
        parse_centroid(right_centroid_json),
    ) {
        (Some(_), Some(_)) => clamp01(1.0 - (region_distance_ft(left_centroid_json, right_centroid_json) / 10.0)),
        _ => 0.0,
    }
}

pub(crate) fn region_distance_ft(left_centroid_json: &str, right_centroid_json: &str) -> f64 {
    let (left, right) = match (
        parse_centroid(left_centroid_json),
        parse_centroid(right_centroid_json),
    ) {
        (Some(left), Some(right)) => (left, right),
        _ => return 0.0,
    };
    let dx = left[0] - right[0];
    let dy = left[1] - right[1];
    let dz = left[2] - right[2];
    (dx * dx + dy * dy + dz * dz).sqrt()
}

pub(crate) fn clearance_to_obstacles(
    region: &WorldRegionRow,
    obstacles: &[WorldRegionRow],
) -> f64 {
    let mut nearest = f64::INFINITY;
    for obstacle in obstacles {
        let distance = region_distance_ft(&region.centroid_json, &obstacle.centroid_json);
        if distance > 0.0 {
            nearest = nearest.min(distance);
        }
    }
    if nearest.is_finite() {
        nearest
    } else {
        0.0
    }
}

pub(crate) fn obstacle_proximity_penalty(
    left: &WorldRegionRow,
    right: &WorldRegionRow,
    obstacles: &[WorldRegionRow],
) -> f64 {
    let edge_clearance = clearance_to_obstacles(left, obstacles)
        .min(clearance_to_obstacles(right, obstacles));
    if edge_clearance <= 0.0 {
        return 1.0;
    }
    clamp01(1.0 - (edge_clearance / 10.0))
}

pub(crate) fn support_balance(left_support: i64, right_support: i64) -> f64 {
    let left = left_support.max(0) as f64;
    let right = right_support.max(0) as f64;
    let larger = left.max(right);
    if larger == 0.0 {
        return 0.0;
    }
    clamp01(left.min(right) / larger)
}

pub(crate) fn region_delta_transform_json(
    left_centroid_json: &str,
    right_centroid_json: &str,
) -> String {
    if let (Some(left), Some(right)) = (
        parse_centroid(left_centroid_json),
        parse_centroid(right_centroid_json),
    ) {
        return serde_json::json!({
            "translation": {
                "x": right[0] - left[0],
                "y": right[1] - left[1],
                "z": right[2] - left[2]
            },
            "rotation": {
                "roll": 0.0,
                "pitch": 0.0,
                "yaw": 0.0
            }
        })
        .to_string();
    }

    serde_json::json!({}).to_string()
}
