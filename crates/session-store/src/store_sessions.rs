//! Session ingest and analysis-result persistence.
//!
//! The methods grouped here cover the front half of the store's lifecycle:
//! cataloguing sessions and their streams (including the MCAP handle, whose
//! payload never leaves the file on disk), recording abstraction epochs and
//! spaces with their sample series, and then storing whatever analysis produces
//! — jobs, graph fragments, belief matrices, message snapshots, world regions
//! and the links between those regions.
//!
//! Every upsert reports the primary key of the row it touched so callers can
//! chain inserts without a follow-up query. No method here creates schema; the
//! tables come from `init_schema`.

use crate::mission_types::{
    AnalysisJobRow, BeliefMatrixRow, GraphFragmentRow, MessageSnapshotRow, RegionLinkRow,
    WorldRegionRow,
};
use crate::rows::{
    enum_to_db_text, map_analysis_job_row, map_belief_matrix_row, map_graph_fragment_row,
    map_message_snapshot_row, map_region_link_row, map_state_sample_row, map_world_region_row,
};
use crate::{
    AbstractStateSample, AbstractStateSampleRow, AbstractionEpochRow, AbstractionEpochUpsert,
    AbstractionSpaceRow, AbstractionSpaceUpsert, AnalysisJobUpsert, BeliefMatrixUpsert,
    GraphFragmentUpsert, MessageSnapshotUpsert, RegionLinkUpsert, SessionRow, SessionStore,
    SessionStreamRow, SessionStreamUpsert, SessionUpsert, WorldRegionUpsert,
};
use qualia_mcap::McapReference;
use rusqlite::{params, OptionalExtension};

impl SessionStore {
    /// Insert a session or refresh the row already keyed by its path; either way
    /// the returned value is the id of the stored session.
    pub fn upsert_session(&self, session: &SessionUpsert) -> rusqlite::Result<i64> {
        let connection = &self.connection;
        let insert = r#"
            INSERT INTO sessions (
                path, filename, media_kind, analysis_kind, status, duration_sec, imported_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(path) DO UPDATE SET
                filename=excluded.filename,
                media_kind=excluded.media_kind,
                analysis_kind=excluded.analysis_kind,
                status=excluded.status,
                duration_sec=excluded.duration_sec,
                imported_at=excluded.imported_at
            "#;
        let values = params![
            session.path, session.filename, session.media_kind, session.analysis_kind,
            session.status, session.duration_sec, session.imported_at
        ];
        connection.execute(insert, values)?;

        let id_of_path = "SELECT id FROM sessions WHERE path = ?1";
        connection.query_row(id_of_path, params![session.path], |row| row.get(0))
    }

    /// All catalogue rows, newest import first, with filename as the tiebreak.
    pub fn list_sessions(&self) -> rusqlite::Result<Vec<SessionRow>> {
        let connection = &self.connection;
        let select = r#"
            SELECT id, path, filename, media_kind, analysis_kind, status, duration_sec, imported_at
            FROM sessions
            ORDER BY imported_at DESC, filename ASC
            "#;
        let mut stmt = connection.prepare(select)?;

        let mapped = stmt.query_map([], |row| {
            Ok(SessionRow {
                id: row.get("id")?,
                path: row.get("path")?,
                filename: row.get("filename")?,
                media_kind: row.get("media_kind")?,
                analysis_kind: row.get("analysis_kind")?,
                status: row.get("status")?,
                duration_sec: row.get("duration_sec")?,
                imported_at: row.get("imported_at")?,
            })
        })?;
        mapped.collect()
    }

    /// Look one catalogue row up by primary key; an unknown id yields `None`.
    pub fn session_by_id(&self, id: i64) -> rusqlite::Result<Option<SessionRow>> {
        let connection = &self.connection;
        let select = r#"
            SELECT id, path, filename, media_kind, analysis_kind, status, duration_sec, imported_at
            FROM sessions
            WHERE id = ?1
            "#;

        connection
            .query_row(select, params![id], |row| {
                Ok(SessionRow {
                    id: row.get("id")?,
                    path: row.get("path")?,
                    filename: row.get("filename")?,
                    media_kind: row.get("media_kind")?,
                    analysis_kind: row.get("analysis_kind")?,
                    status: row.get("status")?,
                    duration_sec: row.get("duration_sec")?,
                    imported_at: row.get("imported_at")?,
                })
            })
            .optional()
    }

    /// Attach a stream to a session, or overwrite the one already stored under
    /// the same `(session_id, stream_key)` pair.
    pub fn upsert_stream(&self, stream: &SessionStreamUpsert) -> rusqlite::Result<i64> {
        let connection = &self.connection;
        let insert = r#"
            INSERT INTO session_streams (
                session_id, stream_key, stream_kind, role, path, sync_group, metadata_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(session_id, stream_key) DO UPDATE SET
                stream_kind=excluded.stream_kind,
                role=excluded.role,
                path=excluded.path,
                sync_group=excluded.sync_group,
                metadata_json=excluded.metadata_json
            "#;
        let values = params![
            stream.session_id, stream.stream_key, stream.stream_kind, stream.role, stream.path,
            stream.sync_group, stream.metadata_json
        ];
        connection.execute(insert, values)?;

        let select = r#"
            SELECT id FROM session_streams
            WHERE session_id = ?1 AND stream_key = ?2
            "#;
        connection.query_row(select, params![stream.session_id, stream.stream_key], |row| {
            row.get(0)
        })
    }

    /// Store the durable handle for a session's MCAP recording as an evidence
    /// stream. Only the reference and inventory are written to SQLite — sensor
    /// payloads stay in the file and are deliberately never copied in.
    pub fn upsert_mcap_reference(
        &self, session_id: i64, reference: &McapReference,
    ) -> rusqlite::Result<i64> {
        let metadata_json = serde_json::to_string(reference).map_err(|error| {
            rusqlite::Error::ToSqlConversionFailure(Box::new(error))
        })?;

        let stream = SessionStreamUpsert {
            session_id, stream_key: "arena_mcap".to_string(),
            stream_kind: "mcap".to_string(), role: "evidence".to_string(),
            path: reference.path.clone(), sync_group: "arena".to_string(), metadata_json,
        };
        self.upsert_stream(&stream)
    }

    /// Streams of one session, ordered by role and then stream key.
    pub fn list_streams(&self, session_id: i64) -> rusqlite::Result<Vec<SessionStreamRow>> {
        let connection = &self.connection;
        let select = r#"
            SELECT id, session_id, stream_key, stream_kind, role, path, sync_group, metadata_json
            FROM session_streams
            WHERE session_id = ?1
            ORDER BY role ASC, stream_key ASC
            "#;
        let mut stmt = connection.prepare(select)?;

        let mapped = stmt.query_map(params![session_id], |row| {
            Ok(SessionStreamRow {
                id: row.get("id")?,
                session_id: row.get("session_id")?,
                stream_key: row.get("stream_key")?,
                stream_kind: row.get("stream_kind")?,
                role: row.get("role")?,
                path: row.get("path")?,
                sync_group: row.get("sync_group")?,
                metadata_json: row.get("metadata_json")?,
            })
        })?;
        mapped.collect()
    }

    /// Upsert an abstraction epoch, whose identity is the session, epoch index,
    /// layer name and abstraction family together.
    pub fn upsert_epoch(&self, epoch: &AbstractionEpochUpsert) -> rusqlite::Result<i64> {
        let connection = &self.connection;
        let insert = r#"
            INSERT INTO abstraction_epochs (
                session_id, epoch_index, layer_name, abstraction_family, status, created_at, completed_at, summary_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            ON CONFLICT(session_id, epoch_index, layer_name, abstraction_family) DO UPDATE SET
                status=excluded.status,
                created_at=excluded.created_at,
                completed_at=excluded.completed_at,
                summary_json=excluded.summary_json
            "#;
        let values = params![
            epoch.session_id, epoch.epoch_index, epoch.layer_name, epoch.abstraction_family,
            epoch.status, epoch.created_at, epoch.completed_at, epoch.summary_json
        ];
        connection.execute(insert, values)?;

        let select = "SELECT id FROM abstraction_epochs WHERE session_id = ?1 AND epoch_index = ?2 AND layer_name = ?3 AND abstraction_family = ?4";
        let key = params![epoch.session_id, epoch.epoch_index, epoch.layer_name, epoch.abstraction_family];
        connection.query_row(select, key, |row| row.get(0))
    }

    /// Upsert a space inside an epoch, keyed by the space's abstraction name.
    pub fn upsert_space(&self, space: &AbstractionSpaceUpsert) -> rusqlite::Result<i64> {
        let connection = &self.connection;
        let insert = r#"
            INSERT INTO abstraction_spaces (
                epoch_id, space_family, abstraction_name, source_kind, representation_kind, uncertainty_kind, dimensionality, schema_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            ON CONFLICT(epoch_id, abstraction_name) DO UPDATE SET
                space_family=excluded.space_family,
                source_kind=excluded.source_kind,
                representation_kind=excluded.representation_kind,
                uncertainty_kind=excluded.uncertainty_kind,
                dimensionality=excluded.dimensionality,
                schema_json=excluded.schema_json
            "#;
        let values = params![
            space.epoch_id, space.space_family, space.abstraction_name, space.source_kind,
            space.representation_kind, space.uncertainty_kind, space.dimensionality,
            space.schema_json
        ];
        connection.execute(insert, values)?;

        let select = "SELECT id FROM abstraction_spaces WHERE epoch_id = ?1 AND abstraction_name = ?2";
        connection.query_row(select, params![space.epoch_id, space.abstraction_name], |row| {
            row.get(0)
        })
    }

    /// Replace the complete sample series stored for one `(epoch, space)` pair.
    /// The purge and the reload share a transaction, so an aborted load leaves
    /// the previously stored series untouched.
    pub fn replace_state_samples(
        &self, epoch_id: i64, space_id: i64, samples: &[AbstractStateSample],
    ) -> rusqlite::Result<()> {
        let connection = &self.connection;
        let tx = connection.unchecked_transaction()?;

        let purge = "DELETE FROM abstract_state_samples WHERE epoch_id = ?1 AND space_id = ?2";
        tx.execute(purge, params![epoch_id, space_id])?;

        let insert = r#"
            INSERT INTO abstract_state_samples (
                epoch_id, space_id, step, timestamp_sec, symbol_key, payload_json, confidence, sample_hash
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            "#;
        {
            let mut stmt = tx.prepare(insert)?;
            for sample in samples {
                stmt.execute(params![
                    epoch_id, space_id, sample.step, sample.timestamp_sec, sample.symbol_key,
                    sample.payload_json, sample.confidence, sample.sample_hash
                ])?;
            }
        }

        tx.commit()
    }

    /// The session imported most recently, or `None` on an empty catalogue.
    pub fn latest_session(&self) -> rusqlite::Result<Option<SessionRow>> {
        let connection = &self.connection;
        let select = r#"
            SELECT id, path, filename, media_kind, analysis_kind, status, duration_sec, imported_at
            FROM sessions
            ORDER BY imported_at DESC, filename ASC
            LIMIT 1
            "#;

        connection
            .query_row(select, [], |row| {
                Ok(SessionRow {
                    id: row.get("id")?,
                    path: row.get("path")?,
                    filename: row.get("filename")?,
                    media_kind: row.get("media_kind")?,
                    analysis_kind: row.get("analysis_kind")?,
                    status: row.get("status")?,
                    duration_sec: row.get("duration_sec")?,
                    imported_at: row.get("imported_at")?,
                })
            })
            .optional()
    }

    /// Epochs of a session, ascending by index, layer name and family.
    pub fn list_epochs(&self, session_id: i64) -> rusqlite::Result<Vec<AbstractionEpochRow>> {
        let connection = &self.connection;
        let select = r#"
            SELECT id, session_id, epoch_index, layer_name, abstraction_family, status,
                   created_at, completed_at, summary_json
            FROM abstraction_epochs
            WHERE session_id = ?1
            ORDER BY epoch_index ASC, layer_name ASC, abstraction_family ASC
            "#;
        let mut stmt = connection.prepare(select)?;

        let mapped = stmt.query_map(params![session_id], |row| {
            Ok(AbstractionEpochRow {
                id: row.get("id")?,
                session_id: row.get("session_id")?,
                epoch_index: row.get("epoch_index")?,
                layer_name: row.get("layer_name")?,
                abstraction_family: row.get("abstraction_family")?,
                status: row.get("status")?,
                created_at: row.get("created_at")?,
                completed_at: row.get("completed_at")?,
                summary_json: row.get("summary_json")?,
            })
        })?;
        mapped.collect()
    }

    /// Spaces of one epoch, ordered by family and then abstraction name.
    pub fn list_spaces(&self, epoch_id: i64) -> rusqlite::Result<Vec<AbstractionSpaceRow>> {
        let connection = &self.connection;
        let select = r#"
            SELECT id, epoch_id, space_family, abstraction_name, source_kind,
                   representation_kind, uncertainty_kind, dimensionality, schema_json
            FROM abstraction_spaces
            WHERE epoch_id = ?1
            ORDER BY space_family ASC, abstraction_name ASC
            "#;
        let mut stmt = connection.prepare(select)?;

        let mapped = stmt.query_map(params![epoch_id], |row| {
            Ok(AbstractionSpaceRow {
                id: row.get("id")?,
                epoch_id: row.get("epoch_id")?,
                space_family: row.get("space_family")?,
                abstraction_name: row.get("abstraction_name")?,
                source_kind: row.get("source_kind")?,
                representation_kind: row.get("representation_kind")?,
                uncertainty_kind: row.get("uncertainty_kind")?,
                dimensionality: row.get("dimensionality")?,
                schema_json: row.get("schema_json")?,
            })
        })?;
        mapped.collect()
    }

    /// Samples of one space in ascending step order, optionally narrowed by a
    /// step window and capped at `limit` rows (never fewer than one).
    pub fn list_state_samples(
        &self,
        space_id: i64, start_step: Option<i64>, end_step: Option<i64>, limit: Option<usize>,
    ) -> rusqlite::Result<Vec<AbstractStateSampleRow>> {
        let connection = &self.connection;
        let base = r#"
            SELECT epoch_id, space_id, step, timestamp_sec, symbol_key, payload_json, confidence, sample_hash
            FROM abstract_state_samples
            WHERE space_id = ?1
            "#;
        let mut query = String::from(base);

        // Each bound present in the filter claims the next positional slot, so a
        // window without a lower bound shifts the upper bound to ?2.
        if start_step.is_some() {
            query.push_str(" AND step >= ?2");
        }
        if end_step.is_some() {
            let slot = if start_step.is_some() { "?3" } else { "?2" };
            query.push_str(&format!(" AND step <= {}", slot));
        }
        query.push_str(" ORDER BY step ASC");
        if let Some(cap) = limit {
            query.push_str(&format!(" LIMIT {}", cap.max(1)));
        }

        let mut stmt = connection.prepare(&query)?;
        let mapped = match (start_step, end_step) {
            (Some(low), Some(high)) => {
                stmt.query_map(params![space_id, low, high], map_state_sample_row)?
            }
            (Some(low), None) => stmt.query_map(params![space_id, low], map_state_sample_row)?,
            (None, Some(high)) => stmt.query_map(params![space_id, high], map_state_sample_row)?,
            (None, None) => stmt.query_map(params![space_id], map_state_sample_row)?,
        };
        mapped.collect()
    }

    /// Record an analysis job under the natural key `(session, job kind,
    /// requested_at)`, so re-issuing the same request refreshes that row.
    pub fn upsert_analysis_job(&self, job: &AnalysisJobUpsert) -> rusqlite::Result<i64> {
        let connection = &self.connection;
        let kind = enum_to_db_text(&job.job_kind)?;
        let state = enum_to_db_text(&job.status)?;

        let insert = r#"
            INSERT INTO analysis_jobs (
                session_id, environment_id, job_kind, status, requested_at, started_at, completed_at,
                window_start_sec, window_end_sec, spec_json, summary_json, failure_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
            ON CONFLICT(session_id, job_kind, requested_at) DO UPDATE SET
                environment_id=excluded.environment_id,
                status=excluded.status,
                started_at=excluded.started_at,
                completed_at=excluded.completed_at,
                window_start_sec=excluded.window_start_sec,
                window_end_sec=excluded.window_end_sec,
                spec_json=excluded.spec_json,
                summary_json=excluded.summary_json,
                failure_json=excluded.failure_json
            "#;
        let values = params![
            job.session_id, job.environment_id, kind, state, job.requested_at, job.started_at,
            job.completed_at, job.window_start_sec, job.window_end_sec, job.spec_json,
            job.summary_json, job.failure_json
        ];
        connection.execute(insert, values)?;

        let select = "SELECT id FROM analysis_jobs WHERE session_id = ?1 AND job_kind = ?2 AND requested_at = ?3";
        connection.query_row(select, params![job.session_id, kind, job.requested_at], |row| {
            row.get(0)
        })
    }

    /// Jobs recorded against a session, newest request first.
    pub fn list_analysis_jobs(&self, session_id: i64) -> rusqlite::Result<Vec<AnalysisJobRow>> {
        let connection = &self.connection;
        let select = r#"
            SELECT id, session_id, environment_id, job_kind, status, requested_at, started_at,
                   completed_at, window_start_sec, window_end_sec, spec_json, summary_json, failure_json
            FROM analysis_jobs
            WHERE session_id = ?1
            ORDER BY requested_at DESC, id DESC
            "#;
        let mut stmt = connection.prepare(select)?;

        let mapped = stmt.query_map(params![session_id], map_analysis_job_row)?;
        mapped.collect()
    }

    /// Upsert a factor-graph fragment, keyed by its job and fragment key. The
    /// stored value is the fragment's structural summary, not the graph body.
    pub fn upsert_graph_fragment(&self, fragment: &GraphFragmentUpsert) -> rusqlite::Result<i64> {
        let connection = &self.connection;
        let kind = enum_to_db_text(&fragment.graph_kind)?;
        let form = enum_to_db_text(&fragment.graph_form)?;
        let exactness = enum_to_db_text(&fragment.exactness)?;

        let insert = r#"
            INSERT INTO graph_fragments (
                analysis_job_id, session_id, epoch_id, stream_key, fragment_key, graph_kind, graph_form,
                exactness, variable_count, factor_count, tree_width, root_variable_key,
                window_start_sec, window_end_sec, summary_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
            ON CONFLICT(analysis_job_id, fragment_key) DO UPDATE SET
                session_id=excluded.session_id,
                epoch_id=excluded.epoch_id,
                stream_key=excluded.stream_key,
                graph_kind=excluded.graph_kind,
                graph_form=excluded.graph_form,
                exactness=excluded.exactness,
                variable_count=excluded.variable_count,
                factor_count=excluded.factor_count,
                tree_width=excluded.tree_width,
                root_variable_key=excluded.root_variable_key,
                window_start_sec=excluded.window_start_sec,
                window_end_sec=excluded.window_end_sec,
                summary_json=excluded.summary_json
            "#;
        let values = params![
            fragment.analysis_job_id, fragment.session_id, fragment.epoch_id, fragment.stream_key,
            fragment.fragment_key, kind, form, exactness, fragment.variable_count,
            fragment.factor_count, fragment.tree_width, fragment.root_variable_key,
            fragment.window_start_sec, fragment.window_end_sec, fragment.summary_json
        ];
        connection.execute(insert, values)?;

        let select = "SELECT id FROM graph_fragments WHERE analysis_job_id = ?1 AND fragment_key = ?2";
        let key = params![fragment.analysis_job_id, fragment.fragment_key];
        connection.query_row(select, key, |row| row.get(0))
    }

    /// Fragments produced for one session, ordered by window start.
    pub fn list_graph_fragments(&self, session_id: i64) -> rusqlite::Result<Vec<GraphFragmentRow>> {
        let connection = &self.connection;
        let select = r#"
            SELECT id, analysis_job_id, session_id, epoch_id, stream_key, fragment_key, graph_kind,
                   graph_form, exactness, variable_count, factor_count, tree_width, root_variable_key,
                   window_start_sec, window_end_sec, summary_json
            FROM graph_fragments
            WHERE session_id = ?1
            ORDER BY window_start_sec ASC, fragment_key ASC
            "#;
        let mut stmt = connection.prepare(select)?;

        let mapped = stmt.query_map(params![session_id], map_graph_fragment_row)?;
        mapped.collect()
    }

    /// A single fragment by primary key, or `None` when no such row exists.
    pub fn graph_fragment_by_id(
        &self, fragment_id: i64,
    ) -> rusqlite::Result<Option<GraphFragmentRow>> {
        let connection = &self.connection;
        let select = r#"
            SELECT id, analysis_job_id, session_id, epoch_id, stream_key, fragment_key, graph_kind,
                   graph_form, exactness, variable_count, factor_count, tree_width, root_variable_key,
                   window_start_sec, window_end_sec, summary_json
            FROM graph_fragments
            WHERE id = ?1
            "#;
        let mut stmt = connection.prepare(select)?;

        stmt.query_row(params![fragment_id], map_graph_fragment_row)
            .optional()
    }

    /// Upsert one variable's marginal within a fragment, keyed by fragment and
    /// variable key.
    pub fn upsert_belief_matrix(&self, matrix: &BeliefMatrixUpsert) -> rusqlite::Result<i64> {
        let connection = &self.connection;
        let domain = enum_to_db_text(&matrix.domain_kind)?;

        let insert = r#"
            INSERT INTO belief_matrices (
                fragment_id, variable_key, domain_kind, normalization_error, entropy,
                max_state_key, values_json, matrix_hash
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            ON CONFLICT(fragment_id, variable_key) DO UPDATE SET
                domain_kind=excluded.domain_kind,
                normalization_error=excluded.normalization_error,
                entropy=excluded.entropy,
                max_state_key=excluded.max_state_key,
                values_json=excluded.values_json,
                matrix_hash=excluded.matrix_hash
            "#;
        let values = params![
            matrix.fragment_id, matrix.variable_key, domain, matrix.normalization_error,
            matrix.entropy, matrix.max_state_key, matrix.values_json, matrix.matrix_hash
        ];
        connection.execute(insert, values)?;

        let select = "SELECT id FROM belief_matrices WHERE fragment_id = ?1 AND variable_key = ?2";
        connection.query_row(select, params![matrix.fragment_id, matrix.variable_key], |row| {
            row.get(0)
        })
    }

    /// Marginals stored for a fragment, in variable-key order.
    pub fn list_belief_matrices(&self, fragment_id: i64) -> rusqlite::Result<Vec<BeliefMatrixRow>> {
        let connection = &self.connection;
        let select = r#"
            SELECT id, fragment_id, variable_key, domain_kind, normalization_error, entropy,
                   max_state_key, values_json, matrix_hash
            FROM belief_matrices
            WHERE fragment_id = ?1
            ORDER BY variable_key ASC
            "#;
        let mut stmt = connection.prepare(select)?;

        let mapped = stmt.query_map(params![fragment_id], map_belief_matrix_row)?;
        mapped.collect()
    }

    /// Upsert a message snapshot, keyed by fragment, edge, direction and
    /// iteration index.
    pub fn upsert_message_snapshot(
        &self, snapshot: &MessageSnapshotUpsert,
    ) -> rusqlite::Result<i64> {
        let connection = &self.connection;
        let direction = enum_to_db_text(&snapshot.direction)?;

        let insert = r#"
            INSERT INTO message_snapshots (
                fragment_id, edge_key, direction, iteration_index, source_node_key,
                target_node_key, values_json, residual_norm, message_hash
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            ON CONFLICT(fragment_id, edge_key, direction, iteration_index) DO UPDATE SET
                source_node_key=excluded.source_node_key,
                target_node_key=excluded.target_node_key,
                values_json=excluded.values_json,
                residual_norm=excluded.residual_norm,
                message_hash=excluded.message_hash
            "#;
        let values = params![
            snapshot.fragment_id, snapshot.edge_key, direction, snapshot.iteration_index,
            snapshot.source_node_key, snapshot.target_node_key, snapshot.values_json,
            snapshot.residual_norm, snapshot.message_hash
        ];
        connection.execute(insert, values)?;

        let select = "SELECT id FROM message_snapshots WHERE fragment_id = ?1 AND edge_key = ?2 AND direction = ?3 AND iteration_index = ?4";
        let key = params![
            snapshot.fragment_id, snapshot.edge_key, direction, snapshot.iteration_index
        ];
        connection.query_row(select, key, |row| row.get(0))
    }

    /// Snapshots of a fragment, ordered by iteration and then edge.
    pub fn list_message_snapshots(
        &self, fragment_id: i64,
    ) -> rusqlite::Result<Vec<MessageSnapshotRow>> {
        let connection = &self.connection;
        let select = r#"
            SELECT id, fragment_id, edge_key, direction, iteration_index, source_node_key,
                   target_node_key, values_json, residual_norm, message_hash
            FROM message_snapshots
            WHERE fragment_id = ?1
            ORDER BY iteration_index ASC, edge_key ASC
            "#;
        let mut stmt = connection.prepare(select)?;

        let mapped = stmt.query_map(params![fragment_id], map_message_snapshot_row)?;
        mapped.collect()
    }

    /// Upsert a curated world region, keyed by session and region key.
    pub fn upsert_world_region(&self, region: &WorldRegionUpsert) -> rusqlite::Result<i64> {
        let connection = &self.connection;
        let kind = enum_to_db_text(&region.region_kind)?;

        let insert = r#"
            INSERT INTO world_regions (
                environment_id, session_id, source_fragment_id, region_key, region_kind,
                support_point_count, confidence, centroid_json, bounds_json,
                signature_hash, metadata_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
            ON CONFLICT(session_id, region_key) DO UPDATE SET
                environment_id=excluded.environment_id,
                source_fragment_id=excluded.source_fragment_id,
                region_kind=excluded.region_kind,
                support_point_count=excluded.support_point_count,
                confidence=excluded.confidence,
                centroid_json=excluded.centroid_json,
                bounds_json=excluded.bounds_json,
                signature_hash=excluded.signature_hash,
                metadata_json=excluded.metadata_json
            "#;
        let values = params![
            region.environment_id, region.session_id, region.source_fragment_id, region.region_key,
            kind, region.support_point_count, region.confidence, region.centroid_json,
            region.bounds_json, region.signature_hash, region.metadata_json
        ];
        connection.execute(insert, values)?;

        let select = "SELECT id FROM world_regions WHERE session_id = ?1 AND region_key = ?2";
        connection.query_row(select, params![region.session_id, region.region_key], |row| {
            row.get(0)
        })
    }

    /// Regions curated from one session, in region-key order.
    pub fn list_world_regions(&self, session_id: i64) -> rusqlite::Result<Vec<WorldRegionRow>> {
        let connection = &self.connection;
        let select = r#"
            SELECT id, environment_id, session_id, source_fragment_id, region_key, region_kind,
                   support_point_count, confidence, centroid_json, bounds_json, signature_hash,
                   metadata_json
            FROM world_regions
            WHERE session_id = ?1
            ORDER BY region_key ASC
            "#;
        let mut stmt = connection.prepare(select)?;

        let mapped = stmt.query_map(params![session_id], map_world_region_row)?;
        mapped.collect()
    }

    /// Regions attributed to an environment, grouped by session then region key.
    pub fn list_world_regions_for_environment(
        &self, environment_id: i64,
    ) -> rusqlite::Result<Vec<WorldRegionRow>> {
        let connection = &self.connection;
        let select = r#"
            SELECT id, environment_id, session_id, source_fragment_id, region_key, region_kind,
                   support_point_count, confidence, centroid_json, bounds_json, signature_hash,
                   metadata_json
            FROM world_regions
            WHERE environment_id = ?1
            ORDER BY session_id ASC, region_key ASC
            "#;
        let mut stmt = connection.prepare(select)?;

        let mapped = stmt.query_map(params![environment_id], map_world_region_row)?;
        mapped.collect()
    }

    /// Upsert a link between two regions, keyed by both endpoints and the link
    /// kind.
    pub fn upsert_region_link(&self, link: &RegionLinkUpsert) -> rusqlite::Result<i64> {
        let connection = &self.connection;
        let kind = enum_to_db_text(&link.link_kind)?;
        let state = enum_to_db_text(&link.state)?;

        let insert = r#"
            INSERT INTO region_links (
                left_region_id, right_region_id, link_kind, score, relative_transform_json,
                contradiction_score, state, evidence_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            ON CONFLICT(left_region_id, right_region_id, link_kind) DO UPDATE SET
                score=excluded.score,
                relative_transform_json=excluded.relative_transform_json,
                contradiction_score=excluded.contradiction_score,
                state=excluded.state,
                evidence_json=excluded.evidence_json
            "#;
        let values = params![
            link.left_region_id, link.right_region_id, kind, link.score,
            link.relative_transform_json, link.contradiction_score, state, link.evidence_json
        ];
        connection.execute(insert, values)?;

        let select = "SELECT id FROM region_links WHERE left_region_id = ?1 AND right_region_id = ?2 AND link_kind = ?3";
        connection.query_row(select, params![link.left_region_id, link.right_region_id, kind], |row| {
            row.get(0)
        })
    }

    /// Links touching a region on either endpoint, strongest score first.
    pub fn list_region_links(&self, region_id: i64) -> rusqlite::Result<Vec<RegionLinkRow>> {
        let connection = &self.connection;
        let select = r#"
            SELECT id, left_region_id, right_region_id, link_kind, score, relative_transform_json,
                   contradiction_score, state, evidence_json
            FROM region_links
            WHERE left_region_id = ?1 OR right_region_id = ?1
            ORDER BY score DESC, id ASC
            "#;
        let mut stmt = connection.prepare(select)?;

        let mapped = stmt.query_map(params![region_id], map_region_link_row)?;
        mapped.collect()
    }
}
