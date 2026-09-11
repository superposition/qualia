//! Persistence for the replication layer.
//!
//! This module owns five tables: the append-only op log, the materialized
//! per-namespace registers, the append-only log entries, the replica registry
//! and the per-peer consumption cursors. The op log's `seq` column (SQLite's
//! rowid order) defines replay order, so every reader here sorts by it
//! ascending. Enum-valued columns are persisted in the text form produced by
//! `qualia-sync-types`, keeping disk and memory vocabularies identical.

use crate::rows::{enum_from_db_text, enum_to_db_text, hlc_order_key};
use crate::SessionStore;
use qualia_sync_types::{
    HlcTimestamp, ReplicaRole, ReplicaTrustState, SyncApplyStatus, SyncBody, SyncEnvelope,
};
use qualia_sync_types::{SyncMaterializedState, SyncNamespace};
use rusqlite::types::Value as SqlValue;
use rusqlite::{params, params_from_iter, OptionalExtension};
use serde_json::Value;
use serde::{Deserialize, Serialize};

/// One op-log row: the received envelope plus how far applying it got.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SyncOpRow {
    pub seq: i64, pub op_id: String,
    pub schema_version: String, pub replica_id: String,
    pub replica_role: ReplicaRole, pub run_id: String,
    pub counter: u64, pub timestamp_hlc: HlcTimestamp,
    pub namespace: SyncNamespace, pub key: String,
    pub body: SyncBody, pub status: SyncApplyStatus,
    pub status_detail: String, pub received_at: String,
}

/// Materialized state of one `(namespace, key)` slot after replay.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SyncStateRow {
    pub namespace: SyncNamespace, pub key: String,
    pub state: SyncMaterializedState, pub updated_hlc: HlcTimestamp,
    pub last_op_id: String,
}

/// One immutable entry of an append-only namespace.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SyncAppendEntryRow {
    pub namespace: SyncNamespace, pub key: String,
    pub entry_id: String, pub value: Value,
    pub timestamp_hlc: HlcTimestamp, pub source_op_id: String,
    pub replica_id: String,
}

/// A replica registered for synchronization.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncReplicaRow {
    pub replica_id: String, pub replica_role: ReplicaRole,
    pub display_name: String, pub endpoint: String,
    pub trust_state: ReplicaTrustState, pub enabled: bool,
    pub capabilities_json: String, pub metadata_json: String,
    pub last_seen_hlc: Option<HlcTimestamp>, pub last_seen_at: Option<String>,
    pub last_sync_seq: i64, pub updated_at: String,
}

/// How far this replica has consumed a peer's log.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncPeerCursorRow {
    pub peer_replica_id: String, pub remote_seq: i64,
    pub local_applied_seq: i64, pub remote_latest_seq: i64,
    pub updated_at: String,
}

/// Caller-supplied replica fields. The observation columns (`last_seen_*`,
/// `last_sync_seq`) are deliberately absent: those move only through the
/// dedicated seen-marker path.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncReplicaUpsert {
    pub replica_id: String, pub replica_role: ReplicaRole,
    pub display_name: String, pub endpoint: String,
    pub trust_state: ReplicaTrustState, pub enabled: bool,
    pub capabilities_json: String, pub metadata_json: String,
    pub updated_at: String,
}

/// Result of appending to the op log. `inserted` is false when the op id was
/// already present; `seq` is the occupying row either way.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncOpInsertResult {
    pub inserted: bool, pub seq: i64,
}

/// Audit rollup for one replica.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncOpAuditReplicaSummary {
    pub replica_id: String, pub total_ops: i64,
    pub accepted_ops: i64, pub stored_only_ops: i64,
    pub rejected_ops: i64,
}

/// Audit rollup for one namespace.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncOpAuditNamespaceSummary {
    pub namespace: SyncNamespace, pub total_ops: i64,
    pub accepted_ops: i64, pub stored_only_ops: i64,
    pub rejected_ops: i64,
}

/// Whole-log audit rollup with per-replica and per-namespace breakdowns.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncOpAuditSummary {
    pub total_ops: i64, pub accepted_ops: i64,
    pub stored_only_ops: i64, pub rejected_ops: i64,
    pub by_replica: Vec<SyncOpAuditReplicaSummary>,
    pub by_namespace: Vec<SyncOpAuditNamespaceSummary>,
}

/// Ops that were applied, or recognized as an already-applied duplicate.
///
/// `SUM` over zero rows decodes as `NULL`, so every aggregate is coalesced:
/// an empty — or entirely filtered-out — op log must report zero counts
/// rather than fail while decoding.
const ACCEPTED_SYNC_OPS_SQL: &str =
    "COALESCE(SUM(CASE WHEN status IN ('applied', 'duplicate') THEN 1 ELSE 0 END), 0)";
/// Ops retained for audit that never touched materialized state.
const STORED_ONLY_SYNC_OPS_SQL: &str =
    "COALESCE(SUM(CASE WHEN status = 'stored_only' THEN 1 ELSE 0 END), 0)";
/// Ops refused outright, one bucket for every rejection status.
const REJECTED_SYNC_OPS_SQL: &str = "COALESCE(SUM(CASE WHEN status IN ('rejected_authority', 'blocked_lease', 'validation_failed', 'checkpoint_mismatch') THEN 1 ELSE 0 END), 0)";

/// Serialize a value destined for a `*_json` column.
fn sql_json<T: Serialize>(value: &T) -> rusqlite::Result<String> {
    serde_json::to_string(value)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))
}

/// Prepare `sql`, bind `binds` and decode every row through `decode`.
fn query_rows<T, P, F>(
    connection: &rusqlite::Connection,
    sql: &str,
    binds: P,
    decode: F,
) -> rusqlite::Result<Vec<T>>
where
    P: rusqlite::Params,
    F: FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
{
    let mut statement = connection.prepare(sql)?;
    let decoded = statement.query_map(binds, decode)?;
    let decoded = decoded.collect::<rusqlite::Result<Vec<T>>>()?;
    Ok(decoded)
}

/// Build the `WHERE` fragment shared by the op-log readers and the audit
/// rollups, plus the bound values in matching positional order.
///
/// A `replica_id` that is empty or all whitespace counts as "not filtered"
/// rather than as a literal empty replica id.
fn build_sync_ops_filter_clause(
    after_seq: Option<i64>, namespace: Option<SyncNamespace>,
    replica_id: Option<&str>, status: Option<SyncApplyStatus>,
) -> rusqlite::Result<(String, Vec<SqlValue>)> {
    let mut filters: Vec<String> = Vec::new();
    let mut binds: Vec<SqlValue> = Vec::new();

    if let Some(seq) = after_seq {
        filters.push("seq > ?".to_string());
        binds.push(SqlValue::Integer(seq));
    }
    if let Some(namespace) = namespace {
        filters.push("namespace = ?".to_string());
        binds.push(SqlValue::Text(namespace.to_string()));
    }
    match replica_id {
        Some(id) if !id.trim().is_empty() => {
            filters.push("replica_id = ?".to_string());
            binds.push(SqlValue::Text(id.to_string()));
        }
        _ => {}
    }
    if let Some(status) = status {
        filters.push("status = ?".to_string());
        binds.push(SqlValue::Text(enum_to_db_text(&status)?));
    }

    let sql = match filters.is_empty() {
        true => String::new(),
        false => format!(" WHERE {}", filters.join(" AND ")),
    };
    Ok((sql, binds))
}

impl SessionStore {
    /// Append a received envelope to the op log. Re-receiving an op id changes
    /// nothing; either way the caller learns the op's sequence number.
    pub fn insert_sync_op(
        &self,
        envelope: &SyncEnvelope, status: SyncApplyStatus,
        status_detail: &str, received_at: &str,
    ) -> rusqlite::Result<SyncOpInsertResult> {
        let op_id = envelope.op_id();
        let role_text = enum_to_db_text(&envelope.replica_role)?;
        let status_text = enum_to_db_text(&status)?;
        let body_json = sql_json(&envelope.body)?;

        let written = self.connection.execute(
            r#"
            INSERT INTO sync_ops (
                op_id, schema_version, replica_id, replica_role, run_id, counter,
                timestamp_hlc, namespace, key, body_json, status, status_detail, received_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
            ON CONFLICT(op_id) DO NOTHING
            "#,
            params![
                op_id, envelope.schema_version,
                envelope.replica_id, role_text,
                envelope.run_id, envelope.counter as i64,
                envelope.timestamp_hlc.to_string(), envelope.namespace.to_string(),
                envelope.key, body_json,
                status_text, status_detail, received_at,
            ],
        )?;

        let seq: i64 = self
            .connection
            .query_row("SELECT seq FROM sync_ops WHERE op_id = ?1", params![op_id], |row| {
                row.get(0)
            })?;

        Ok(SyncOpInsertResult {
            inserted: written > 0,
            seq,
        })
    }

    /// Record the outcome of trying to apply an op.
    pub fn update_sync_op_status(
        &self,
        op_id: &str, status: SyncApplyStatus, status_detail: &str,
    ) -> rusqlite::Result<()> {
        let status_text = enum_to_db_text(&status)?;
        self.connection.execute(
            "UPDATE sync_ops SET status = ?2, status_detail = ?3 WHERE op_id = ?1",
            params![op_id, status_text, status_detail],
        )?;
        Ok(())
    }

    /// Fetch a single op by its stable id.
    pub fn get_sync_op(&self, op_id: &str) -> rusqlite::Result<Option<SyncOpRow>> {
        let found = self
            .connection
            .query_row(
                r#"
                SELECT seq, op_id, schema_version, replica_id, replica_role, run_id, counter,
                       timestamp_hlc, namespace, key, body_json, status, status_detail, received_at
                FROM sync_ops
                WHERE op_id = ?1
                "#,
                params![op_id],
                map_sync_op_row,
            )
            .optional()?;
        Ok(found)
    }

    /// Oldest-first page of ops, optionally resuming after a known sequence.
    pub fn list_sync_ops(
        &self,
        after_seq: Option<i64>, namespace: Option<SyncNamespace>, limit: usize,
    ) -> rusqlite::Result<Vec<SyncOpRow>> {
        self.list_sync_ops_filtered(after_seq, namespace, None, None, limit)
    }

    /// Like [`SessionStore::list_sync_ops`], further narrowable by replica and
    /// apply status. The filter values are pushed in the clause order the
    /// builder used, then the page size last.
    pub fn list_sync_ops_filtered(
        &self, after_seq: Option<i64>, namespace: Option<SyncNamespace>,
        replica_id: Option<&str>, status: Option<SyncApplyStatus>,
        limit: usize,
    ) -> rusqlite::Result<Vec<SyncOpRow>> {
        let page = limit.max(1) as i64;
        let (where_clause, mut binds) =
            build_sync_ops_filter_clause(after_seq, namespace, replica_id, status)?;
        binds.push(SqlValue::Integer(page));

        let sql = format!(
            r#"
            SELECT seq, op_id, schema_version, replica_id, replica_role, run_id, counter,
                   timestamp_hlc, namespace, key, body_json, status, status_detail, received_at
            FROM sync_ops
            {where_clause}
            ORDER BY seq ASC
            LIMIT ?
            "#
        );

        let rows = query_rows(&self.connection, &sql, params_from_iter(binds), map_sync_op_row)?;
        Ok(rows)
    }

    /// Roll the filtered op log up into total, per-replica and per-namespace
    /// counts, each split into accepted, stored-only and rejected.
    pub fn summarize_sync_ops(
        &self,
        after_seq: Option<i64>, namespace: Option<SyncNamespace>,
        replica_id: Option<&str>, status: Option<SyncApplyStatus>,
    ) -> rusqlite::Result<SyncOpAuditSummary> {
        let (where_clause, binds) =
            build_sync_ops_filter_clause(after_seq, namespace, replica_id, status)?;

        let totals_sql = format!(
            r#"
            SELECT
                COUNT(*),
                {ACCEPTED_SYNC_OPS_SQL},
                {STORED_ONLY_SYNC_OPS_SQL},
                {REJECTED_SYNC_OPS_SQL}
            FROM sync_ops
            {where_clause}
            "#
        );
        let totals = self.connection.query_row(
            &totals_sql,
            params_from_iter(binds.clone()),
            |row| {
                let counts = (row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?);
                Ok(counts)
            },
        )?;
        let (total_ops, accepted_ops, stored_only_ops, rejected_ops) = totals;

        let replica_sql = format!(
            r#"
            SELECT
                replica_id,
                COUNT(*),
                {ACCEPTED_SYNC_OPS_SQL},
                {STORED_ONLY_SYNC_OPS_SQL},
                {REJECTED_SYNC_OPS_SQL}
            FROM sync_ops
            {where_clause}
            GROUP BY replica_id
            ORDER BY replica_id ASC
            "#
        );
        let by_replica = query_rows(
            &self.connection,
            &replica_sql,
            params_from_iter(binds.clone()),
            |row| {
                let replica_id: String = row.get(0)?;
                let summary = SyncOpAuditReplicaSummary {
                    replica_id, total_ops: row.get(1)?,
                    accepted_ops: row.get(2)?, stored_only_ops: row.get(3)?,
                    rejected_ops: row.get(4)?,
                };
                Ok(summary)
            },
        )?;

        let namespace_sql = format!(
            r#"
            SELECT
                namespace,
                COUNT(*),
                {ACCEPTED_SYNC_OPS_SQL},
                {STORED_ONLY_SYNC_OPS_SQL},
                {REJECTED_SYNC_OPS_SQL}
            FROM sync_ops
            {where_clause}
            GROUP BY namespace
            ORDER BY namespace ASC
            "#
        );
        let by_namespace = query_rows(
            &self.connection,
            &namespace_sql,
            params_from_iter(binds),
            |row| {
                let namespace_text: String = row.get(0)?;
                let summary = SyncOpAuditNamespaceSummary {
                    namespace: enum_from_db_text(namespace_text)?,
                    total_ops: row.get(1)?, accepted_ops: row.get(2)?,
                    stored_only_ops: row.get(3)?, rejected_ops: row.get(4)?,
                };
                Ok(summary)
            },
        )?;

        let summary = SyncOpAuditSummary {
            total_ops, accepted_ops,
            stored_only_ops, rejected_ops,
            by_replica, by_namespace,
        };
        Ok(summary)
    }

    /// Highest sequence number committed so far, or zero on an empty log.
    pub fn latest_sync_seq(&self) -> rusqlite::Result<i64> {
        let latest = self
            .connection
            .query_row("SELECT COALESCE(MAX(seq), 0) FROM sync_ops", [], |row| {
                row.get(0)
            })?;
        Ok(latest)
    }

    /// Write the materialized state of one `(namespace, key)` slot.
    pub fn upsert_sync_state(
        &self,
        namespace: SyncNamespace, key: &str,
        state: &SyncMaterializedState, updated_hlc: HlcTimestamp,
        last_op_id: &str,
    ) -> rusqlite::Result<()> {
        let namespace_text = namespace.to_string();
        let state_json = sql_json(state)?;
        let hlc_text = updated_hlc.to_string();
        self.connection.execute(
            r#"
            INSERT INTO sync_materialized_states (namespace, key, state_json, updated_hlc, last_op_id)
            VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(namespace, key) DO UPDATE SET
                state_json = excluded.state_json,
                updated_hlc = excluded.updated_hlc,
                last_op_id = excluded.last_op_id
            "#,
            params![namespace_text, key, state_json, hlc_text, last_op_id],
        )?;
        Ok(())
    }

    /// Read one materialized slot, if it was ever written.
    pub fn get_sync_state(
        &self, namespace: SyncNamespace, key: &str,
    ) -> rusqlite::Result<Option<SyncStateRow>> {
        let found = self
            .connection
            .query_row(
                r#"
                SELECT namespace, key, state_json, updated_hlc, last_op_id
                FROM sync_materialized_states
                WHERE namespace = ?1 AND key = ?2
                "#,
                params![namespace.to_string(), key],
                map_sync_state_row,
            )
            .optional()?;
        Ok(found)
    }

    /// All materialized slots, optionally restricted to one namespace.
    pub fn list_sync_states(
        &self, namespace: Option<SyncNamespace>,
    ) -> rusqlite::Result<Vec<SyncStateRow>> {
        let rows = match namespace {
            Some(namespace) => query_rows(
                &self.connection,
                r#"
                SELECT namespace, key, state_json, updated_hlc, last_op_id
                FROM sync_materialized_states
                WHERE namespace = ?1
                ORDER BY key ASC
                "#,
                params![namespace.to_string()],
                map_sync_state_row,
            )?,
            None => query_rows(
                &self.connection,
                r#"
                SELECT namespace, key, state_json, updated_hlc, last_op_id
                FROM sync_materialized_states
                ORDER BY namespace ASC, key ASC
                "#,
                [],
                map_sync_state_row,
            )?,
        };
        Ok(rows)
    }

    /// Append one entry to an append-only log, returning whether a new row was
    /// written. The `(namespace, key, entry_id)` triple is the dedup key.
    pub fn insert_sync_append_entry(
        &self,
        namespace: SyncNamespace, key: &str,
        entry_id: &str, value: &Value,
        timestamp_hlc: HlcTimestamp,
        source_op_id: &str, replica_id: &str,
    ) -> rusqlite::Result<bool> {
        let value_json = sql_json(value)?;
        let written = self.connection.execute(
            r#"
            INSERT INTO sync_append_entries (
                namespace, key, entry_id, value_json, timestamp_hlc, source_op_id, replica_id
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(namespace, key, entry_id) DO NOTHING
            "#,
            params![
                namespace.to_string(), key,
                entry_id, value_json,
                timestamp_hlc.to_string(), source_op_id, replica_id,
            ],
        )?;
        Ok(written > 0)
    }

    /// Oldest-first page of append entries, ordered by HLC then entry id.
    ///
    /// Entries are partitioned by namespace, so a key-only lookup is ambiguous;
    /// it yields nothing rather than leaking rows from other namespaces.
    pub fn list_sync_append_entries(
        &self,
        namespace: Option<SyncNamespace>, key: Option<&str>, limit: usize,
    ) -> rusqlite::Result<Vec<SyncAppendEntryRow>> {
        let page = limit.max(1) as i64;
        let order = hlc_order_key("timestamp_hlc");
        let rows = match (namespace, key) {
            (Some(namespace), Some(key)) => query_rows(
                &self.connection,
                &format!(
                    r#"
                SELECT namespace, key, entry_id, value_json, timestamp_hlc, source_op_id, replica_id
                FROM sync_append_entries
                WHERE namespace = ?1 AND key = ?2
                ORDER BY {order}, entry_id ASC
                LIMIT ?3
                "#
                ),
                params![namespace.to_string(), key, page],
                map_sync_append_row,
            )?,
            (Some(namespace), None) => query_rows(
                &self.connection,
                &format!(
                    r#"
                SELECT namespace, key, entry_id, value_json, timestamp_hlc, source_op_id, replica_id
                FROM sync_append_entries
                WHERE namespace = ?1
                ORDER BY {order}, entry_id ASC
                LIMIT ?2
                "#
                ),
                params![namespace.to_string(), page],
                map_sync_append_row,
            )?,
            (None, Some(_)) => Vec::new(),
            (None, None) => query_rows(
                &self.connection,
                &format!(
                    r#"
                SELECT namespace, key, entry_id, value_json, timestamp_hlc, source_op_id, replica_id
                FROM sync_append_entries
                ORDER BY {order}, entry_id ASC
                LIMIT ?1
                "#
                ),
                params![page],
                map_sync_append_row,
            )?,
        };
        Ok(rows)
    }

    /// Insert or refresh a replica's configuration; the observation columns are
    /// left as they are.
    pub fn upsert_sync_replica(&self, replica: &SyncReplicaUpsert) -> rusqlite::Result<()> {
        let role_text = enum_to_db_text(&replica.replica_role)?;
        let trust_text = enum_to_db_text(&replica.trust_state)?;
        let enabled_flag = bool_to_i64(replica.enabled);
        self.connection.execute(
            r#"
            INSERT INTO sync_replicas (
                replica_id, replica_role, display_name, endpoint, trust_state, enabled,
                capabilities_json, metadata_json, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            ON CONFLICT(replica_id) DO UPDATE SET
                replica_role = excluded.replica_role,
                display_name = excluded.display_name,
                endpoint = excluded.endpoint,
                trust_state = excluded.trust_state,
                enabled = excluded.enabled,
                capabilities_json = excluded.capabilities_json,
                metadata_json = excluded.metadata_json,
                updated_at = excluded.updated_at
            "#,
            params![
                replica.replica_id, role_text,
                replica.display_name, replica.endpoint,
                trust_text, enabled_flag,
                replica.capabilities_json, replica.metadata_json, replica.updated_at,
            ],
        )?;
        Ok(())
    }

    /// Fetch a registered replica by id.
    pub fn get_sync_replica(&self, replica_id: &str) -> rusqlite::Result<Option<SyncReplicaRow>> {
        let found = self
            .connection
            .query_row(
                r#"
                SELECT replica_id, replica_role, display_name, endpoint, trust_state, enabled,
                       capabilities_json, metadata_json, last_seen_hlc, last_seen_at,
                       last_sync_seq, updated_at
                FROM sync_replicas
                WHERE replica_id = ?1
                "#,
                params![replica_id],
                map_sync_replica_row,
            )
            .optional()?;
        Ok(found)
    }

    /// Every registered replica, ordered by id.
    pub fn list_sync_replicas(&self) -> rusqlite::Result<Vec<SyncReplicaRow>> {
        query_rows(
            &self.connection,
            r#"
            SELECT replica_id, replica_role, display_name, endpoint, trust_state, enabled,
                   capabilities_json, metadata_json, last_seen_hlc, last_seen_at,
                   last_sync_seq, updated_at
            FROM sync_replicas
            ORDER BY replica_id ASC
            "#,
            [],
            map_sync_replica_row,
        )
    }

    /// Advance a replica's last-seen marker, forwards only: an observation that
    /// is not newer — HLC first, sequence breaking ties — leaves the row alone.
    /// Returns whether the row changed; an unregistered replica returns false.
    pub fn mark_sync_replica_seen(
        &self,
        replica_id: &str, last_seen_hlc: HlcTimestamp,
        last_seen_at: &str, last_sync_seq: i64,
    ) -> rusqlite::Result<bool> {
        let Some(current) = self.get_sync_replica(replica_id)? else {
            return Ok(false);
        };
        let advances = match current.last_seen_hlc {
            Some(previous) => (last_seen_hlc, last_sync_seq) > (previous, current.last_sync_seq),
            None => true,
        };
        if !advances {
            return Ok(false);
        }

        let seen_text = last_seen_hlc.to_string();
        let changed = self.connection.execute(
            r#"
            UPDATE sync_replicas
            SET last_seen_hlc = ?2,
                last_seen_at = ?3,
                last_sync_seq = ?4,
                updated_at = ?3
            WHERE replica_id = ?1
            "#,
            params![replica_id, seen_text, last_seen_at, last_sync_seq],
        )?;
        Ok(changed != 0)
    }

    /// Read the persisted consumption cursor for a peer.
    pub fn get_sync_peer_cursor(
        &self, peer_replica_id: &str,
    ) -> rusqlite::Result<Option<SyncPeerCursorRow>> {
        let found = self
            .connection
            .query_row(
                r#"
                SELECT peer_replica_id, remote_seq, local_applied_seq, remote_latest_seq, updated_at
                FROM sync_peer_cursors
                WHERE peer_replica_id = ?1
                "#,
                params![peer_replica_id],
                map_sync_peer_cursor_row,
            )
            .optional()?;
        Ok(found)
    }

    /// Store a peer cursor. The three sequence columns only ever move forward,
    /// so a stale write cannot rewind progress; `updated_at` always takes the
    /// incoming value.
    pub fn upsert_sync_peer_cursor(&self, cursor: &SyncPeerCursorRow) -> rusqlite::Result<()> {
        self.connection.execute(
            r#"
            INSERT INTO sync_peer_cursors (
                peer_replica_id, remote_seq, local_applied_seq, remote_latest_seq, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(peer_replica_id) DO UPDATE SET
                remote_seq = MAX(sync_peer_cursors.remote_seq, excluded.remote_seq),
                local_applied_seq = MAX(sync_peer_cursors.local_applied_seq, excluded.local_applied_seq),
                remote_latest_seq = MAX(sync_peer_cursors.remote_latest_seq, excluded.remote_latest_seq),
                updated_at = excluded.updated_at
            "#,
            params![
                cursor.peer_replica_id, cursor.remote_seq,
                cursor.local_applied_seq, cursor.remote_latest_seq, cursor.updated_at,
            ],
        )?;
        Ok(())
    }
}

/// Decode an op-log row in the column order every `sync_ops` reader uses.
fn map_sync_op_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SyncOpRow> {
    let counter: i64 = row.get(6)?;
    let timestamp_text: String = row.get(7)?;
    let namespace_text: String = row.get(8)?;
    let body_json: String = row.get(10)?;
    let status_text: String = row.get(11)?;

    let decoded = SyncOpRow {
        seq: row.get(0)?, op_id: row.get(1)?,
        schema_version: row.get(2)?, replica_id: row.get(3)?,
        replica_role: enum_from_db_text(row.get(4)?)?, run_id: row.get(5)?,
        counter: counter as u64, timestamp_hlc: parse_hlc(timestamp_text)?,
        namespace: parse_namespace(namespace_text)?, key: row.get(9)?,
        body: serde_json::from_str(&body_json).map_err(to_from_sql_err)?,
        status: enum_from_db_text(status_text)?, status_detail: row.get(12)?,
        received_at: row.get(13)?,
    };
    Ok(decoded)
}

/// Decode a materialized-state row.
fn map_sync_state_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SyncStateRow> {
    let namespace_text: String = row.get(0)?;
    let state_json: String = row.get(2)?;
    let updated_text: String = row.get(3)?;

    let decoded = SyncStateRow {
        namespace: parse_namespace(namespace_text)?, key: row.get(1)?,
        state: serde_json::from_str(&state_json).map_err(to_from_sql_err)?,
        updated_hlc: parse_hlc(updated_text)?, last_op_id: row.get(4)?,
    };
    Ok(decoded)
}

/// Decode an append-entry row.
fn map_sync_append_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SyncAppendEntryRow> {
    let namespace_text: String = row.get(0)?;
    let value_json: String = row.get(3)?;
    let timestamp_text: String = row.get(4)?;

    let decoded = SyncAppendEntryRow {
        namespace: parse_namespace(namespace_text)?, key: row.get(1)?,
        entry_id: row.get(2)?,
        value: serde_json::from_str(&value_json).map_err(to_from_sql_err)?,
        timestamp_hlc: parse_hlc(timestamp_text)?, source_op_id: row.get(5)?,
        replica_id: row.get(6)?,
    };
    Ok(decoded)
}

/// Decode a replica-registry row.
fn map_sync_replica_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SyncReplicaRow> {
    let enabled_flag: i64 = row.get(5)?;
    let seen_text: Option<String> = row.get(8)?;
    let last_seen_hlc = match seen_text {
        Some(text) => Some(parse_hlc(text)?),
        None => None,
    };

    let decoded = SyncReplicaRow {
        replica_id: row.get(0)?, replica_role: enum_from_db_text(row.get(1)?)?,
        display_name: row.get(2)?, endpoint: row.get(3)?,
        trust_state: enum_from_db_text(row.get(4)?)?, enabled: enabled_flag != 0,
        capabilities_json: row.get(6)?, metadata_json: row.get(7)?,
        last_seen_hlc, last_seen_at: row.get(9)?,
        last_sync_seq: row.get(10)?, updated_at: row.get(11)?,
    };
    Ok(decoded)
}

/// Decode a peer-cursor row.
fn map_sync_peer_cursor_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SyncPeerCursorRow> {
    let peer: String = row.get(0)?;
    let remote: i64 = row.get(1)?;
    let applied: i64 = row.get(2)?;
    let latest: i64 = row.get(3)?;
    let updated: String = row.get(4)?;

    let decoded = SyncPeerCursorRow {
        peer_replica_id: peer, remote_seq: remote,
        local_applied_seq: applied, remote_latest_seq: latest,
        updated_at: updated,
    };
    Ok(decoded)
}

/// Parse a stored HLC text form, attributing failures to a text column.
fn parse_hlc(text: String) -> rusqlite::Result<HlcTimestamp> {
    text.parse().map_err(to_from_sql_err_message)
}

/// Parse a stored namespace text form, attributing failures to a text column.
fn parse_namespace(text: String) -> rusqlite::Result<SyncNamespace> {
    text.parse().map_err(to_from_sql_err_message)
}

/// Wrap a deserialization error as a column-conversion failure.
fn to_from_sql_err<E>(error: E) -> rusqlite::Error
where
    E: std::error::Error + Send + Sync + 'static,
{
    let column = 0;
    let kind = rusqlite::types::Type::Text;
    rusqlite::Error::FromSqlConversionFailure(column, kind, Box::new(error))
}

/// Like [`to_from_sql_err`], for text parsers that report a plain message.
fn to_from_sql_err_message(message: String) -> rusqlite::Error {
    let cause = std::io::Error::new(std::io::ErrorKind::InvalidData, message);
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(cause))
}

/// SQLite stores flags as 0/1 integers; this is the mapping used for `enabled`.
fn bool_to_i64(value: bool) -> i64 {
    match value {
        true => 1,
        false => 0,
    }
}
