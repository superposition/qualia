//! Persistence for the distributed world-model pipeline.
//!
//! Proposals, curator decisions and canonical states live in tables keyed by
//! the identifier carried in the matching `qualia_sync_types` envelope. The
//! numeric tensor states (belief vectors, spline controls, relation factors)
//! share one table and are separated by a `state_kind` discriminator. Payload
//! columns hold JSON, and the schema version is stamped back in when a row is
//! turned into an envelope again.

use qualia_sync_types::{
    BeliefVectorState, CanonicalStateEnvelope, CoachDecision, ProposalEnvelope, RelationFactorState,
    SplineControlState, WORLD_MODEL_SCHEMA_VERSION,
};
use qualia_sync_types::{
    CanonicalKind, CoachDecisionKind, ProposalKind, ProposalStatus,
};
use rusqlite::types::Type;
use rusqlite::{params, OptionalExtension};

use crate::rows::{enum_from_db_text, enum_to_db_text, hlc_order_key};
use crate::SessionStore;

/// Which numeric payload variant a `world_model_tensor_state` row carries.
#[derive(Debug, Clone, Copy)]
pub enum WorldModelTensorStateKind {
    BeliefVector, SplineControl, RelationFactor,
}

/// Values written into the `state_kind` column, one per tensor variant.
const BELIEF_VECTOR_KIND: &str = "belief_vector";
const SPLINE_CONTROL_KIND: &str = "spline_control";
const RELATION_FACTOR_KIND: &str = "relation_factor";

impl WorldModelTensorStateKind {
    /// Text written into the `state_kind` column for this variant.
    fn as_str(self) -> &'static str {
        match self {
            Self::BeliefVector => BELIEF_VECTOR_KIND,
            Self::SplineControl => SPLINE_CONTROL_KIND,
            Self::RelationFactor => RELATION_FACTOR_KIND,
        }
    }
}

/// Serialises a sync-type payload for a JSON column.
fn json_text<T: serde::Serialize>(value: &T) -> rusqlite::Result<String> {
    serde_json::to_string(value).map_err(to_encoding_err)
}

/// Rebuilds a sync-type payload from a JSON column.
fn parse_json<T: for<'de> serde::Deserialize<'de>>(text: &str) -> rusqlite::Result<T> {
    serde_json::from_str(text).map_err(to_decoding_err)
}

/// Rebuilds the hybrid-logical clock stored as text in an ordering column.
fn parse_hlc(text: &str) -> rusqlite::Result<qualia_sync_types::HlcTimestamp> {
    text.parse().map_err(to_timestamp_err)
}

/// A `LIMIT 0` would return nothing, so row caps below one are rounded up.
fn page_limit(limit: usize) -> i64 {
    limit.max(1) as i64
}

/// Prepares `sql` and collects every row produced by `mapper`.
fn query_rows<T, P>(
    connection: &rusqlite::Connection,
    sql: &str,
    parameters: P,
    mapper: fn(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
) -> rusqlite::Result<Vec<T>>
where
    P: rusqlite::Params,
{
    let mut statement = connection.prepare(sql)?;
    let rows = statement
        .query_map(parameters, mapper)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn invalid_data(message: String) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message)
}

fn to_encoding_err<E>(error: E) -> rusqlite::Error
where
    E: std::error::Error + Send + Sync + 'static,
{
    rusqlite::Error::ToSqlConversionFailure(Box::new(error))
}

fn to_decoding_err<E>(error: E) -> rusqlite::Error
where
    E: std::error::Error + Send + Sync + 'static,
{
    rusqlite::Error::FromSqlConversionFailure(0, Type::Text, Box::new(error))
}

fn to_validation_err(message: String) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(invalid_data(message)))
}

fn to_timestamp_err(message: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, Type::Text, Box::new(invalid_data(message)))
}

fn map_world_model_proposal(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProposalEnvelope> {
    let proposal_id = row.get(0)?;
    let kind_text = row.get(1)?;
    let source_replica_id = row.get(2)?;
    let role_text = row.get(3)?;
    let created_at: String = row.get(4)?;
    let status_text = row.get(5)?;
    let belief_weight = row.get(6)?;
    let source_weight = row.get(7)?;
    let mission_relevance = row.get(8)?;
    let confidence = row.get(9)?;
    let lineage_json: String = row.get(10)?;
    let body_json: String = row.get(11)?;
    Ok(ProposalEnvelope {
        schema_version: WORLD_MODEL_SCHEMA_VERSION.to_string(),
        proposal_id,
        proposal_kind: enum_from_db_text(kind_text)?,
        source_replica_id,
        source_replica_role: enum_from_db_text(role_text)?,
        created_at_hlc: parse_hlc(&created_at)?,
        status: enum_from_db_text(status_text)?,
        belief_weight,
        source_weight,
        mission_relevance,
        confidence,
        lineage: parse_json(&lineage_json)?,
        body: parse_json(&body_json)?,
    })
}

fn map_world_model_decision(row: &rusqlite::Row<'_>) -> rusqlite::Result<CoachDecision> {
    let decision_id = row.get(0)?;
    let kind_text = row.get(1)?;
    let curator_replica_id = row.get(2)?;
    let role_text = row.get(3)?;
    let created_at: String = row.get(4)?;
    let targets_json: String = row.get(5)?;
    let outputs_json: String = row.get(6)?;
    let reason: String = row.get(7)?;
    Ok(CoachDecision {
        schema_version: WORLD_MODEL_SCHEMA_VERSION.to_string(),
        decision_id,
        decision_kind: enum_from_db_text(kind_text)?,
        curator_replica_id,
        curator_replica_role: enum_from_db_text(role_text)?,
        created_at_hlc: parse_hlc(&created_at)?,
        target_proposal_ids: parse_json(&targets_json)?,
        output_ids: parse_json(&outputs_json)?,
        reason: (!reason.is_empty()).then_some(reason),
    })
}

fn map_world_model_canonical(row: &rusqlite::Row<'_>) -> rusqlite::Result<CanonicalStateEnvelope> {
    let canonical_id = row.get(0)?;
    let kind_text = row.get(1)?;
    let source_decision_id = row.get(2)?;
    let sources_json: String = row.get(3)?;
    let accepted_at: String = row.get(4)?;
    let status_text = row.get(5)?;
    let body_json: String = row.get(6)?;
    Ok(CanonicalStateEnvelope {
        schema_version: WORLD_MODEL_SCHEMA_VERSION.to_string(),
        canonical_id,
        canonical_kind: enum_from_db_text(kind_text)?,
        source_decision_id,
        source_proposal_ids: parse_json(&sources_json)?,
        accepted_at_hlc: parse_hlc(&accepted_at)?,
        status: enum_from_db_text(status_text)?,
        body: parse_json(&body_json)?,
    })
}

impl SessionStore {
    pub fn upsert_world_model_proposal(
        &self, proposal: &ProposalEnvelope,
    ) -> rusqlite::Result<()> {
        let sql = r#"
            INSERT INTO world_model_proposals (
                proposal_id, proposal_kind, source_replica_id, source_replica_role,
                created_at_hlc, status, belief_weight, source_weight, mission_relevance,
                confidence, lineage_json, body_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
            ON CONFLICT(proposal_id) DO UPDATE SET
                proposal_kind = excluded.proposal_kind,
                source_replica_id = excluded.source_replica_id,
                source_replica_role = excluded.source_replica_role,
                created_at_hlc = excluded.created_at_hlc,
                status = excluded.status,
                belief_weight = excluded.belief_weight,
                source_weight = excluded.source_weight,
                mission_relevance = excluded.mission_relevance,
                confidence = excluded.confidence,
                lineage_json = excluded.lineage_json,
                body_json = excluded.body_json
        "#;
        let kind_text = enum_to_db_text(&proposal.proposal_kind)?;
        let role_text = enum_to_db_text(&proposal.source_replica_role)?;
        let status_text = enum_to_db_text(&proposal.status)?;
        let lineage_json = json_text(&proposal.lineage)?;
        let body_json = json_text(&proposal.body)?;
        let created_at = proposal.created_at_hlc.to_string();
        self.connection.execute(
            sql,
            params![
                proposal.proposal_id,
                kind_text,
                proposal.source_replica_id,
                role_text,
                created_at,
                status_text,
                proposal.belief_weight, proposal.source_weight,
                proposal.mission_relevance, proposal.confidence,
                lineage_json,
                body_json,
            ],
        )?;
        Ok(())
    }

    pub fn get_world_model_proposal(&self, proposal_id: &str)
        -> rusqlite::Result<Option<ProposalEnvelope>> {
        let sql = r#"
            SELECT proposal_id, proposal_kind, source_replica_id, source_replica_role,
                   created_at_hlc, status, belief_weight, source_weight, mission_relevance,
                   confidence, lineage_json, body_json
            FROM world_model_proposals
            WHERE proposal_id = ?1
        "#;
        let found = self
            .connection
            .query_row(sql, params![proposal_id], map_world_model_proposal);
        found.optional()
    }

    pub fn list_world_model_proposals(
        &self, proposal_kind: Option<ProposalKind>, status: Option<ProposalStatus>,
        limit: usize,
    ) -> rusqlite::Result<Vec<ProposalEnvelope>> {
        let limit = page_limit(limit);
        let order = hlc_order_key("created_at_hlc");
        match (proposal_kind, status) {
            (Some(kind), Some(status)) => {
                let sql = format!(
                    r#"
                    SELECT proposal_id, proposal_kind, source_replica_id, source_replica_role,
                           created_at_hlc, status, belief_weight, source_weight, mission_relevance,
                           confidence, lineage_json, body_json
                    FROM world_model_proposals
                    WHERE proposal_kind = ?1 AND status = ?2
                    ORDER BY {order}, proposal_id ASC
                    LIMIT ?3
                "#
                );
                let kind_text = enum_to_db_text(&kind)?;
                let status_text = enum_to_db_text(&status)?;
                query_rows(
                    &self.connection,
                    &sql,
                    params![kind_text, status_text, limit],
                    map_world_model_proposal,
                )
            }
            (Some(kind), None) => {
                let sql = format!(
                    r#"
                    SELECT proposal_id, proposal_kind, source_replica_id, source_replica_role,
                           created_at_hlc, status, belief_weight, source_weight, mission_relevance,
                           confidence, lineage_json, body_json
                    FROM world_model_proposals
                    WHERE proposal_kind = ?1
                    ORDER BY {order}, proposal_id ASC
                    LIMIT ?2
                "#
                );
                let kind_text = enum_to_db_text(&kind)?;
                query_rows(
                    &self.connection,
                    &sql,
                    params![kind_text, limit],
                    map_world_model_proposal,
                )
            }
            (None, Some(status)) => {
                let sql = format!(
                    r#"
                    SELECT proposal_id, proposal_kind, source_replica_id, source_replica_role,
                           created_at_hlc, status, belief_weight, source_weight, mission_relevance,
                           confidence, lineage_json, body_json
                    FROM world_model_proposals
                    WHERE status = ?1
                    ORDER BY {order}, proposal_id ASC
                    LIMIT ?2
                "#
                );
                let status_text = enum_to_db_text(&status)?;
                query_rows(
                    &self.connection,
                    &sql,
                    params![status_text, limit],
                    map_world_model_proposal,
                )
            }
            (None, None) => {
                let sql = format!(
                    r#"
                    SELECT proposal_id, proposal_kind, source_replica_id, source_replica_role,
                           created_at_hlc, status, belief_weight, source_weight, mission_relevance,
                           confidence, lineage_json, body_json
                    FROM world_model_proposals
                    ORDER BY {order}, proposal_id ASC
                    LIMIT ?1
                "#
                );
                query_rows(
                    &self.connection,
                    &sql,
                    params![limit],
                    map_world_model_proposal,
                )
            }
        }
    }

    pub fn update_world_model_proposal_status(&self, proposal_id: &str, status: ProposalStatus)
        -> rusqlite::Result<()> {
        let status_text = enum_to_db_text(&status)?;
        self.connection.execute(
            "UPDATE world_model_proposals SET status = ?2 WHERE proposal_id = ?1",
            params![proposal_id, status_text],
        )?;
        Ok(())
    }

    pub fn insert_world_model_decision(&self, decision: &CoachDecision) -> rusqlite::Result<()> {
        let sql = r#"
            INSERT INTO world_model_decisions (
                decision_id, decision_kind, curator_replica_id, curator_replica_role,
                created_at_hlc, target_proposal_ids_json, output_ids_json, reason
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            ON CONFLICT(decision_id) DO UPDATE SET
                decision_kind = excluded.decision_kind,
                curator_replica_id = excluded.curator_replica_id,
                curator_replica_role = excluded.curator_replica_role,
                created_at_hlc = excluded.created_at_hlc,
                target_proposal_ids_json = excluded.target_proposal_ids_json,
                output_ids_json = excluded.output_ids_json,
                reason = excluded.reason
        "#;
        let kind_text = enum_to_db_text(&decision.decision_kind)?;
        let role_text = enum_to_db_text(&decision.curator_replica_role)?;
        let targets_json = json_text(&decision.target_proposal_ids)?;
        let outputs_json = json_text(&decision.output_ids)?;
        let created_at = decision.created_at_hlc.to_string();
        self.connection.execute(
            sql,
            params![
                decision.decision_id,
                kind_text,
                decision.curator_replica_id,
                role_text,
                created_at,
                targets_json,
                outputs_json,
                decision.reason.as_deref().unwrap_or(""),
            ],
        )?;
        Ok(())
    }

    pub fn list_world_model_decisions(
        &self, decision_kind: Option<CoachDecisionKind>, limit: usize,
    ) -> rusqlite::Result<Vec<CoachDecision>> {
        let limit = page_limit(limit);
        let order = hlc_order_key("created_at_hlc");
        match decision_kind {
            Some(kind) => {
                let sql = format!(
                    r#"
                    SELECT decision_id, decision_kind, curator_replica_id, curator_replica_role,
                           created_at_hlc, target_proposal_ids_json, output_ids_json, reason
                    FROM world_model_decisions
                    WHERE decision_kind = ?1
                    ORDER BY {order}, decision_id ASC
                    LIMIT ?2
                "#
                );
                let kind_text = enum_to_db_text(&kind)?;
                query_rows(
                    &self.connection,
                    &sql,
                    params![kind_text, limit],
                    map_world_model_decision,
                )
            }
            None => {
                let sql = format!(
                    r#"
                    SELECT decision_id, decision_kind, curator_replica_id, curator_replica_role,
                           created_at_hlc, target_proposal_ids_json, output_ids_json, reason
                    FROM world_model_decisions
                    ORDER BY {order}, decision_id ASC
                    LIMIT ?1
                "#
                );
                query_rows(
                    &self.connection,
                    &sql,
                    params![limit],
                    map_world_model_decision,
                )
            }
        }
    }

    pub fn upsert_world_model_canonical(&self, canonical: &CanonicalStateEnvelope)
        -> rusqlite::Result<()> {
        let sql = r#"
            INSERT INTO world_model_canonical (
                canonical_id, canonical_kind, source_decision_id, source_proposal_ids_json,
                accepted_at_hlc, status, body_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(canonical_id) DO UPDATE SET
                canonical_kind = excluded.canonical_kind,
                source_decision_id = excluded.source_decision_id,
                source_proposal_ids_json = excluded.source_proposal_ids_json,
                accepted_at_hlc = excluded.accepted_at_hlc,
                status = excluded.status,
                body_json = excluded.body_json
        "#;
        let kind_text = enum_to_db_text(&canonical.canonical_kind)?;
        let status_text = enum_to_db_text(&canonical.status)?;
        let sources_json = json_text(&canonical.source_proposal_ids)?;
        let body_json = json_text(&canonical.body)?;
        let accepted_at = canonical.accepted_at_hlc.to_string();
        self.connection.execute(
            sql,
            params![
                canonical.canonical_id,
                kind_text,
                canonical.source_decision_id,
                sources_json,
                accepted_at,
                status_text,
                body_json,
            ],
        )?;
        Ok(())
    }

    pub fn list_world_model_canonical(
        &self, canonical_kind: Option<CanonicalKind>, limit: usize,
    ) -> rusqlite::Result<Vec<CanonicalStateEnvelope>> {
        let limit = page_limit(limit);
        let order = hlc_order_key("accepted_at_hlc");
        match canonical_kind {
            Some(kind) => {
                let sql = format!(
                    r#"
                    SELECT canonical_id, canonical_kind, source_decision_id, source_proposal_ids_json,
                           accepted_at_hlc, status, body_json
                    FROM world_model_canonical
                    WHERE canonical_kind = ?1
                    ORDER BY {order}, canonical_id ASC
                    LIMIT ?2
                "#
                );
                let kind_text = enum_to_db_text(&kind)?;
                query_rows(
                    &self.connection,
                    &sql,
                    params![kind_text, limit],
                    map_world_model_canonical,
                )
            }
            None => {
                let sql = format!(
                    r#"
                    SELECT canonical_id, canonical_kind, source_decision_id, source_proposal_ids_json,
                           accepted_at_hlc, status, body_json
                    FROM world_model_canonical
                    ORDER BY {order}, canonical_id ASC
                    LIMIT ?1
                "#
                );
                query_rows(
                    &self.connection,
                    &sql,
                    params![limit],
                    map_world_model_canonical,
                )
            }
        }
    }

    pub fn upsert_world_model_belief_vector(
        &self, state: &BeliefVectorState, updated_at_hlc: &str,
    ) -> rusqlite::Result<()> {
        state.validate().map_err(to_validation_err)?;
        let kind = WorldModelTensorStateKind::BeliefVector;
        let key = state.key.as_str();
        let profile_id = state.profile_id.as_str();
        self.upsert_world_model_tensor_state(key, kind, profile_id, state, updated_at_hlc)
    }

    pub fn list_world_model_belief_vectors(&self) -> rusqlite::Result<Vec<BeliefVectorState>> {
        let kind = WorldModelTensorStateKind::BeliefVector;
        self.list_world_model_tensor_state(kind)
    }

    pub fn upsert_world_model_spline_control(
        &self, state: &SplineControlState, updated_at_hlc: &str,
    ) -> rusqlite::Result<()> {
        state.validate().map_err(to_validation_err)?;
        let kind = WorldModelTensorStateKind::SplineControl;
        let key = state.key.as_str();
        let profile_id = state.profile_id.as_str();
        self.upsert_world_model_tensor_state(key, kind, profile_id, state, updated_at_hlc)
    }

    pub fn list_world_model_spline_controls(&self) -> rusqlite::Result<Vec<SplineControlState>> {
        let kind = WorldModelTensorStateKind::SplineControl;
        self.list_world_model_tensor_state(kind)
    }

    pub fn upsert_world_model_relation_factor(
        &self, state: &RelationFactorState, updated_at_hlc: &str,
    ) -> rusqlite::Result<()> {
        state.validate().map_err(to_validation_err)?;
        let kind = WorldModelTensorStateKind::RelationFactor;
        let key = state.key.as_str();
        let profile_id = state.profile_id.as_str();
        self.upsert_world_model_tensor_state(key, kind, profile_id, state, updated_at_hlc)
    }

    pub fn list_world_model_relation_factors(&self) -> rusqlite::Result<Vec<RelationFactorState>> {
        let kind = WorldModelTensorStateKind::RelationFactor;
        self.list_world_model_tensor_state(kind)
    }
}

impl SessionStore {
    fn upsert_world_model_tensor_state<T: serde::Serialize>(
        &self, key: &str, kind: WorldModelTensorStateKind,
        profile_id: &str, payload: &T, updated_at_hlc: &str,
    ) -> rusqlite::Result<()> {
        let sql = r#"
            INSERT INTO world_model_tensor_state (
                state_key, state_kind, profile_id, payload_json, updated_at_hlc
            ) VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(state_key) DO UPDATE SET
                state_kind = excluded.state_kind,
                profile_id = excluded.profile_id,
                payload_json = excluded.payload_json,
                updated_at_hlc = excluded.updated_at_hlc
        "#;
        let payload_json = json_text(payload)?;
        self.connection.execute(
            sql,
            params![key, kind.as_str(), profile_id, payload_json, updated_at_hlc],
        )?;
        Ok(())
    }

    fn list_world_model_tensor_state<T: for<'de> serde::Deserialize<'de>>(
        &self, kind: WorldModelTensorStateKind,
    ) -> rusqlite::Result<Vec<T>> {
        let order = hlc_order_key("updated_at_hlc");
        let sql = format!(
            r#"
            SELECT payload_json
            FROM world_model_tensor_state
            WHERE state_kind = ?1
            ORDER BY {order}, state_key ASC
        "#
        );
        let mut statement = self.connection.prepare(&sql)?;
        let payloads = statement
            .query_map(params![kind.as_str()], |row| {
                parse_json(&row.get::<_, String>(0)?)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(payloads)
    }
}
