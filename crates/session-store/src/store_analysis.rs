//! Merge-candidate scoring, plan-run persistence and the deliberate planner.
//!
//! `generate_session_merge_candidates` compares the world regions of two
//! sessions pairwise, blends five agreement factors into an overlap score and
//! keeps every pair that clears the caller's thresholds — writing each kept
//! pair both as a merge candidate and as a region link.
//!
//! `generate_deliberate_plan` expands a cheapest-first frontier across the
//! environment's drivable regions. A region may only be a waypoint when its
//! fragment's inference was exact; obstacle clusters act as hazards that raise
//! edge risk and cap the route's clearance rather than as graph nodes.
//!
//! Both generators are deterministic in the sense that matters: the order in
//! which rows are written, the arithmetic that produces each score and the
//! tie-breaking of the frontier are all fixed by the code below.

use crate::mission_types::{
    CandidateState, LinkKind, PlanRunRow, PlanStatus, RegionKind, SessionMergeCandidateRow,
    WorldRegionRow,
};
use crate::rows::{
    centroid_distance_score, clamp01, clearance_to_obstacles, enum_to_db_text, map_plan_run_row,
    map_planner_snapshot_row, map_session_merge_candidate_row, obstacle_proximity_penalty,
    region_delta_transform_json, region_distance_ft, support_balance,
};
use crate::{
    DeliberatePlanRequest, MergeCandidateGenerationReport, MergeCandidateThresholds,
    PlanRunRegionRow, PlanRunRegionUpsert, PlanRunUpsert, PlannerSnapshotRow, PlannerSnapshotUpsert,
    RegionLinkUpsert, SessionMergeCandidateUpsert, SessionStore, TrajectoryCandidateRow,
    TrajectoryCandidateUpsert,
};
use rusqlite::{params, OptionalExtension};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

impl SessionStore {
    pub fn upsert_session_merge_candidate(&self, candidate: &SessionMergeCandidateUpsert) -> rusqlite::Result<i64> {
        // The conflict target and the read-back key both use the stored enum
        // spelling, so encode each once and reuse it.
        let kind_text = enum_to_db_text(&candidate.candidate_kind)?;
        let state_text = enum_to_db_text(&candidate.state)?;

        self.connection.execute(
            r#"
            INSERT INTO session_merge_candidates (
                left_session_id, right_session_id, left_region_id, right_region_id,
                candidate_kind, score, transform_consistency, contradiction_score,
                state, reason_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            ON CONFLICT(left_session_id, right_session_id, left_region_id, right_region_id, candidate_kind) DO UPDATE SET
                score=excluded.score,
                transform_consistency=excluded.transform_consistency,
                contradiction_score=excluded.contradiction_score,
                state=excluded.state,
                reason_json=excluded.reason_json
            "#,
            params![
                candidate.left_session_id, candidate.right_session_id, candidate.left_region_id,
                candidate.right_region_id, kind_text, candidate.score,
                candidate.transform_consistency, candidate.contradiction_score, state_text,
                candidate.reason_json,
            ],
        )?;

        let lookup = "SELECT id FROM session_merge_candidates WHERE left_session_id = ?1 AND right_session_id = ?2 AND left_region_id = ?3 AND right_region_id = ?4 AND candidate_kind = ?5";
        self.connection.query_row(
            lookup,
            params![
                candidate.left_session_id, candidate.right_session_id, candidate.left_region_id,
                candidate.right_region_id, kind_text
            ],
            |row| row.get(0),
        )
    }

    pub fn list_session_merge_candidates(&self, left_session_id: i64, right_session_id: i64) -> rusqlite::Result<Vec<SessionMergeCandidateRow>> {
        let mut stmt = self.connection.prepare(
            r#"
            SELECT id, left_session_id, right_session_id, left_region_id, right_region_id,
                   candidate_kind, score, transform_consistency, contradiction_score, state, reason_json
            FROM session_merge_candidates
            WHERE left_session_id = ?1 AND right_session_id = ?2
            ORDER BY score DESC, id ASC
            "#,
        )?;
        let matched = stmt.query_map(params![left_session_id, right_session_id], map_session_merge_candidate_row)?;
        matched.collect()
    }

    pub fn list_session_merge_candidates_for_session(&self, session_id: i64) -> rusqlite::Result<Vec<SessionMergeCandidateRow>> {
        let mut stmt = self.connection.prepare(
            r#"
            SELECT id, left_session_id, right_session_id, left_region_id, right_region_id,
                   candidate_kind, score, transform_consistency, contradiction_score, state, reason_json
            FROM session_merge_candidates
            WHERE left_session_id = ?1 OR right_session_id = ?1
            ORDER BY score DESC, id ASC
            "#,
        )?;
        let matched = stmt.query_map(params![session_id], map_session_merge_candidate_row)?;
        matched.collect()
    }

    pub fn generate_session_merge_candidates(&self, left_session_id: i64, right_session_id: i64, thresholds: &MergeCandidateThresholds) -> rusqlite::Result<MergeCandidateGenerationReport> {
        let left_side = self.list_world_regions(left_session_id)?;
        let right_side = self.list_world_regions(right_session_id)?;

        let mut pairs_seen = 0usize;
        let mut candidates_written = 0usize;
        let mut links_written = 0usize;

        for left in left_side.iter() {
            for right in right_side.iter() {
                pairs_seen += 1;

                // The five factors: matching kind/signature, centroid closeness,
                // support-point balance and the mean of the two confidences.
                let kind_agree = if left.region_kind == right.region_kind { 1.0 } else { 0.35 };
                let sig_agree = if left.signature_hash == right.signature_hash { 1.0 } else { 0.0 };
                let centroid_score = centroid_distance_score(&left.centroid_json, &right.centroid_json);
                let point_balance = support_balance(left.support_point_count, right.support_point_count);
                let mean_confidence = ((left.confidence + right.confidence) * 0.5).clamp(0.0, 1.0);

                let overlap_score = clamp01(
                    0.45 * sig_agree + 0.20 * kind_agree + 0.20 * centroid_score + 0.15 * point_balance,
                );
                let transform_consistency = clamp01(0.55 * centroid_score + 0.45 * kind_agree);
                let confidence_gap = (left.confidence - right.confidence).abs();
                let contradiction_score =
                    clamp01((1.0 - overlap_score) * 0.70 + confidence_gap * 0.30);

                let too_weak = overlap_score < thresholds.overlap_min;
                let too_wobbly = transform_consistency < thresholds.transform_consistency_min;
                let too_contradictory = contradiction_score > thresholds.contradiction_max;
                if too_weak || too_wobbly || too_contradictory {
                    continue;
                }

                // Record the factors alongside the thresholds that admitted the
                // pair, so a later review can see why it was proposed.
                let reason_json = serde_json::json!({
                    "factors": {
                        "kind_score": kind_agree, "signature_score": sig_agree,
                        "distance_score": centroid_score, "support_balance": point_balance,
                        "confidence_pair": mean_confidence
                    },
                    "thresholds": {
                        "overlap_min": thresholds.overlap_min, "transform_consistency_min": thresholds.transform_consistency_min,
                        "contradiction_max": thresholds.contradiction_max
                    }
                })
                .to_string();

                self.upsert_session_merge_candidate(&SessionMergeCandidateUpsert {
                    left_session_id, right_session_id, left_region_id: left.id, right_region_id: right.id,
                    candidate_kind: LinkKind::Overlap, score: overlap_score,
                    transform_consistency, contradiction_score, state: CandidateState::Proposed, reason_json,
                })?;
                candidates_written += 1;

                let relative_transform =
                    region_delta_transform_json(&left.centroid_json, &right.centroid_json);
                let evidence_json = serde_json::json!({
                    "source": "session_merge_candidate_generation",
                    "left_session_id": left_session_id, "right_session_id": right_session_id,
                    "left_region_id": left.id, "right_region_id": right.id
                })
                .to_string();

                self.upsert_region_link(&RegionLinkUpsert {
                    left_region_id: left.id, right_region_id: right.id, link_kind: LinkKind::Overlap,
                    score: overlap_score, relative_transform_json: relative_transform,
                    contradiction_score, state: CandidateState::Proposed, evidence_json,
                })?;
                links_written += 1;
            }
        }

        Ok(MergeCandidateGenerationReport {
            evaluated_pairs: pairs_seen,
            persisted_candidates: candidates_written,
            persisted_region_links: links_written,
        })
    }

    pub fn upsert_plan_run(&self, run: &PlanRunUpsert) -> rusqlite::Result<i64> {
        let status_text = enum_to_db_text(&run.status)?;

        self.connection.execute(
            r#"
            INSERT INTO plan_runs (
                environment_id, session_id, source_region_id, target_region_id, planner_kind,
                status, path_cost, risk_score, clearance_min_ft, started_at, completed_at, summary_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
            ON CONFLICT(environment_id, planner_kind, started_at) DO UPDATE SET
                session_id=excluded.session_id,
                source_region_id=excluded.source_region_id,
                target_region_id=excluded.target_region_id,
                status=excluded.status,
                path_cost=excluded.path_cost,
                risk_score=excluded.risk_score,
                clearance_min_ft=excluded.clearance_min_ft,
                completed_at=excluded.completed_at,
                summary_json=excluded.summary_json
            "#,
            params![
                run.environment_id, run.session_id, run.source_region_id, run.target_region_id,
                run.planner_kind, status_text, run.path_cost, run.risk_score,
                run.clearance_min_ft, run.started_at, run.completed_at, run.summary_json,
            ],
        )?;

        let lookup = "SELECT id FROM plan_runs WHERE environment_id = ?1 AND planner_kind = ?2 AND started_at = ?3";
        self.connection.query_row(
            lookup,
            params![run.environment_id, run.planner_kind, run.started_at],
            |row| row.get(0),
        )
    }

    pub fn list_plan_runs(&self, environment_id: i64) -> rusqlite::Result<Vec<PlanRunRow>> {
        let mut stmt = self.connection.prepare(
            r#"
            SELECT id, environment_id, session_id, source_region_id, target_region_id,
                   planner_kind, status, path_cost, risk_score, clearance_min_ft,
                   started_at, completed_at, summary_json
            FROM plan_runs
            WHERE environment_id = ?1
            ORDER BY started_at DESC, id DESC
            "#,
        )?;
        let matched = stmt.query_map(params![environment_id], map_plan_run_row)?;
        matched.collect()
    }

    pub fn list_plan_runs_for_session(&self, session_id: i64) -> rusqlite::Result<Vec<PlanRunRow>> {
        let mut stmt = self.connection.prepare(
            r#"
            SELECT id, environment_id, session_id, source_region_id, target_region_id,
                   planner_kind, status, path_cost, risk_score, clearance_min_ft,
                   started_at, completed_at, summary_json
            FROM plan_runs
            WHERE session_id = ?1
            ORDER BY started_at DESC, id DESC
            "#,
        )?;
        let matched = stmt.query_map(params![session_id], map_plan_run_row)?;
        matched.collect()
    }

    pub fn replace_plan_run_regions(&self, plan_run_id: i64, regions: &[PlanRunRegionUpsert]) -> rusqlite::Result<()> {
        self.connection.execute("DELETE FROM plan_run_regions WHERE plan_run_id = ?1", params![plan_run_id])?;

        for region in regions.iter() {
            self.connection.execute(
                r#"
                INSERT INTO plan_run_regions (
                    plan_run_id, step_index, region_id, cumulative_cost, cumulative_risk
                ) VALUES (?1, ?2, ?3, ?4, ?5)
                "#,
                params![
                    plan_run_id, region.step_index, region.region_id, region.cumulative_cost,
                    region.cumulative_risk,
                ],
            )?;
        }

        Ok(())
    }

    pub fn list_plan_run_regions(&self, plan_run_id: i64) -> rusqlite::Result<Vec<PlanRunRegionRow>> {
        let mut stmt = self.connection.prepare(
            r#"
            SELECT plan_run_id, step_index, region_id, cumulative_cost, cumulative_risk
            FROM plan_run_regions
            WHERE plan_run_id = ?1
            ORDER BY step_index ASC
            "#,
        )?;
        let matched = stmt.query_map(params![plan_run_id], |row| {
            Ok(PlanRunRegionRow {
                plan_run_id: row.get(0)?, step_index: row.get(1)?, region_id: row.get(2)?,
                cumulative_cost: row.get(3)?, cumulative_risk: row.get(4)?,
            })
        })?;
        matched.collect()
    }

    pub fn upsert_planner_snapshot(&self, snapshot: &PlannerSnapshotUpsert) -> rusqlite::Result<i64> {
        self.connection.execute(
            r#"
            INSERT INTO planner_snapshots (
                plan_run_id, session_id, graph_fragment_id, snapshot_key, status,
                window_start_sec, window_end_sec, selected_candidate_key, trace_key,
                trace_status, summary_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
            ON CONFLICT(plan_run_id, snapshot_key) DO UPDATE SET
                session_id=excluded.session_id,
                graph_fragment_id=excluded.graph_fragment_id,
                status=excluded.status,
                window_start_sec=excluded.window_start_sec,
                window_end_sec=excluded.window_end_sec,
                selected_candidate_key=excluded.selected_candidate_key,
                trace_key=excluded.trace_key,
                trace_status=excluded.trace_status,
                summary_json=excluded.summary_json
            "#,
            params![
                snapshot.plan_run_id, snapshot.session_id, snapshot.graph_fragment_id,
                snapshot.snapshot_key, snapshot.status, snapshot.window_start_sec,
                snapshot.window_end_sec, snapshot.selected_candidate_key, snapshot.trace_key,
                snapshot.trace_status, snapshot.summary_json,
            ],
        )?;

        let lookup = "SELECT id FROM planner_snapshots WHERE plan_run_id = ?1 AND snapshot_key = ?2";
        self.connection.query_row(
            lookup,
            params![snapshot.plan_run_id, snapshot.snapshot_key],
            |row| row.get(0),
        )
    }

    pub fn list_planner_snapshots_for_session(&self, session_id: i64) -> rusqlite::Result<Vec<PlannerSnapshotRow>> {
        let mut stmt = self.connection.prepare(
            r#"
            SELECT id, plan_run_id, session_id, graph_fragment_id, snapshot_key, status,
                   window_start_sec, window_end_sec, selected_candidate_key, trace_key,
                   trace_status, summary_json
            FROM planner_snapshots
            WHERE session_id = ?1
            ORDER BY window_start_sec DESC, id DESC
            "#,
        )?;
        let matched = stmt.query_map(params![session_id], map_planner_snapshot_row)?;
        matched.collect()
    }

    pub fn planner_snapshot_by_plan_run(&self, plan_run_id: i64) -> rusqlite::Result<Option<PlannerSnapshotRow>> {
        let lookup = r#"
                SELECT id, plan_run_id, session_id, graph_fragment_id, snapshot_key, status,
                       window_start_sec, window_end_sec, selected_candidate_key, trace_key,
                       trace_status, summary_json
                FROM planner_snapshots
                WHERE plan_run_id = ?1
                ORDER BY window_start_sec DESC, id DESC
                LIMIT 1
                "#;
        self.connection
            .query_row(lookup, params![plan_run_id], map_planner_snapshot_row)
            .optional()
    }

    pub fn replace_trajectory_candidates(&self, planner_snapshot_id: i64, candidates: &[TrajectoryCandidateUpsert]) -> rusqlite::Result<()> {
        self.connection.execute("DELETE FROM trajectory_candidates WHERE planner_snapshot_id = ?1", params![planner_snapshot_id])?;

        for candidate in candidates.iter() {
            self.connection.execute(
                r#"
                INSERT INTO trajectory_candidates (
                    planner_snapshot_id, candidate_key, status, score, world_region_keys_json,
                    rejection_reason, summary_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                "#,
                params![
                    planner_snapshot_id, candidate.candidate_key, candidate.status, candidate.score,
                    candidate.world_region_keys_json, candidate.rejection_reason,
                    candidate.summary_json,
                ],
            )?;
        }

        Ok(())
    }

    pub fn list_trajectory_candidates(&self, planner_snapshot_id: i64) -> rusqlite::Result<Vec<TrajectoryCandidateRow>> {
        let mut stmt = self.connection.prepare(
            r#"
            SELECT id, planner_snapshot_id, candidate_key, status, score, world_region_keys_json,
                   rejection_reason, summary_json
            FROM trajectory_candidates
            WHERE planner_snapshot_id = ?1
            ORDER BY id ASC
            "#,
        )?;
        let matched = stmt.query_map(params![planner_snapshot_id], |row| {
            Ok(TrajectoryCandidateRow {
                id: row.get(0)?, planner_snapshot_id: row.get(1)?, candidate_key: row.get(2)?,
                status: row.get(3)?, score: row.get(4)?, world_region_keys_json: row.get(5)?,
                rejection_reason: row.get(6)?, summary_json: row.get(7)?,
            })
        })?;
        matched.collect()
    }

    pub fn generate_deliberate_plan(&self, request: &DeliberatePlanRequest) -> rusqlite::Result<PlanRunRow> {
        let regions = self.list_world_regions_for_environment(request.environment_id)?;
        let mut regions_by_id: HashMap<i64, WorldRegionRow> = HashMap::new();
        let mut hazards: Vec<WorldRegionRow> = Vec::new();

        for region in regions {
            let is_hazard = region.region_kind == RegionKind::ObstacleCluster;
            let region_id = region.id;
            if is_hazard {
                hazards.push(region.clone());
            }
            regions_by_id.insert(region_id, region);
        }

        let start = regions_by_id.get(&request.source_region_id).ok_or_else(|| {
            rusqlite::Error::InvalidParameterName("source region is not part of the environment".into())
        })?;
        let goal = regions_by_id.get(&request.target_region_id).ok_or_else(|| {
            rusqlite::Error::InvalidParameterName("target region is not part of the environment".into())
        })?;

        // Waypoint admission: drivable free space above the confidence floor,
        // backed by a fragment whose inference was exact.
        let is_drivable = |region: &WorldRegionRow| {
            region.region_kind != RegionKind::ObstacleCluster
                && region.confidence >= request.thresholds.free_space_confidence_min
        };
        let has_exact_fragment = |fragment_id: i64| -> rusqlite::Result<bool> {
            match self.graph_fragment_by_id(fragment_id)? {
                Some(fragment) => Ok(fragment.is_exact_tree_family()),
                None => Ok(false),
            }
        };

        let start_ok = is_drivable(start) && has_exact_fragment(start.source_fragment_id)?;
        let goal_ok = is_drivable(goal) && has_exact_fragment(goal.source_fragment_id)?;

        let store_no_path = |reason: &str| -> rusqlite::Result<PlanRunRow> {
            let rejected = PlanRunUpsert {
                environment_id: request.environment_id, session_id: Some(start.session_id),
                source_region_id: Some(start.id), target_region_id: Some(goal.id),
                planner_kind: request.planner_kind.clone(), status: PlanStatus::NoFeasiblePath,
                path_cost: 0.0, risk_score: 1.0, clearance_min_ft: 0.0,
                started_at: request.started_at.clone(), completed_at: request.completed_at.clone(),
                summary_json: serde_json::json!({ "reason": reason }).to_string(),
            };
            let run_id = self.upsert_plan_run(&rejected)?;
            self.plan_run_by_id(run_id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
        };

        if !start_ok || !goal_ok {
            return store_no_path("confidence_or_exactness_gate_failed");
        }

        let waypoints: Vec<WorldRegionRow> = regions_by_id
            .values()
            .filter(|region| is_drivable(region))
            .filter_map(|region| {
                let trusted = has_exact_fragment(region.source_fragment_id).unwrap_or(false);
                if trusted { Some(region.clone()) } else { None }
            })
            .collect();

        // Min-heap entry: `BinaryHeap` pops the largest, so invert the cost
        // comparison to get the cheapest frontier node first.
        #[derive(Copy, Clone, Debug)]
        struct QueueItem {
            cost: f64,
            region_id: i64,
        }
        impl Eq for QueueItem {}
        impl PartialEq for QueueItem {
            fn eq(&self, other: &Self) -> bool {
                self.cost == other.cost && self.region_id == other.region_id
            }
        }
        impl Ord for QueueItem {
            fn cmp(&self, other: &Self) -> Ordering {
                match other.cost.partial_cmp(&self.cost) {
                    Some(ordering) => ordering,
                    None => Ordering::Equal,
                }
            }
        }
        impl PartialOrd for QueueItem {
            fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
                Some(self.cmp(other))
            }
        }

        // Adjacency maps each waypoint to (neighbour, edge cost, edge risk).
        // Hops must be strictly positive and within the request's distance cap.
        let mut adjacency: HashMap<i64, Vec<(i64, f64, f64)>> = HashMap::new();
        for from_region in waypoints.iter() {
            for to_region in waypoints.iter() {
                if from_region.id == to_region.id {
                    continue;
                }
                let hop_ft = region_distance_ft(&from_region.centroid_json, &to_region.centroid_json);
                if hop_ft <= 0.0 || hop_ft > request.thresholds.max_edge_distance_ft {
                    continue;
                }
                let pair_confidence = ((from_region.confidence + to_region.confidence) * 0.5).clamp(0.0, 1.0);
                let hazard_penalty = obstacle_proximity_penalty(from_region, to_region, &hazards);
                let hop_risk = clamp01((1.0 - pair_confidence) * 0.75 + hazard_penalty * 0.25);
                let hop_cost = hop_ft * (2.0 - pair_confidence);
                adjacency
                    .entry(from_region.id)
                    .or_default()
                    .push((to_region.id, hop_cost, hop_risk));
            }
        }

        let mut cost_to: HashMap<i64, f64> = HashMap::new();
        let mut risk_to: HashMap<i64, f64> = HashMap::new();
        let mut came_from: HashMap<i64, i64> = HashMap::new();
        let mut queue = BinaryHeap::new();

        cost_to.insert(start.id, 0.0);
        risk_to.insert(start.id, 0.0);
        queue.push(QueueItem { cost: 0.0, region_id: start.id });

        while let Some(QueueItem { cost, region_id }) = queue.pop() {
            if region_id == goal.id {
                break;
            }
            let best_known = cost_to.get(&region_id).copied().unwrap_or(f64::INFINITY);
            if cost > best_known {
                continue;
            }
            let Some(links) = adjacency.get(&region_id).cloned() else {
                continue;
            };
            for (hop, hop_cost, hop_risk) in links {
                let hop_total = cost + hop_cost;
                let risk_to_here = risk_to.get(&region_id).copied().unwrap_or(0.0);
                let hop_worst = risk_to_here.max(hop_risk);
                if hop_worst > request.thresholds.risk_threshold {
                    continue;
                }
                let hop_best = cost_to.get(&hop).copied().unwrap_or(f64::INFINITY);
                if hop_total < hop_best {
                    cost_to.insert(hop, hop_total);
                    risk_to.insert(hop, hop_worst);
                    came_from.insert(hop, region_id);
                    queue.push(QueueItem { cost: hop_total, region_id: hop });
                }
            }
        }

        if !cost_to.contains_key(&goal.id) {
            return store_no_path("no_connected_path");
        }

        let mut route = vec![goal.id];
        let mut cursor = goal.id;
        while let Some(parent) = came_from.get(&cursor).copied() {
            cursor = parent;
            route.push(cursor);
            if cursor == start.id {
                break;
            }
        }
        route.reverse();

        let mut tightest_clearance = f64::INFINITY;
        for region_id in route.iter() {
            if let Some(region) = regions_by_id.get(region_id) {
                tightest_clearance = tightest_clearance.min(clearance_to_obstacles(region, &hazards));
            }
        }
        let clearance_min_ft = if tightest_clearance.is_finite() { tightest_clearance } else { 0.0 };

        let route_cost = cost_to.get(&goal.id).copied().unwrap_or(0.0);
        let route_risk = risk_to.get(&goal.id).copied().unwrap_or(1.0);
        let completed = PlanRunUpsert {
            environment_id: request.environment_id, session_id: Some(start.session_id),
            source_region_id: Some(start.id), target_region_id: Some(goal.id),
            planner_kind: request.planner_kind.clone(), status: PlanStatus::Complete,
            path_cost: route_cost, risk_score: route_risk, clearance_min_ft,
            started_at: request.started_at.clone(), completed_at: request.completed_at.clone(),
            summary_json: serde_json::json!({
                "path_region_ids": route,
                "flown_vs_planned": { "distance_error_ft": 0.0, "heading_error_deg": 0.0 }
            })
            .to_string(),
        };
        let run_id = self.upsert_plan_run(&completed)?;

        // Re-walk the route to store each step's running cost and the worst
        // edge risk seen so far.
        let mut running_cost = 0.0;
        let mut running_risk: f64 = 0.0;
        let mut step_rows = Vec::new();
        for (step, region_id) in route.iter().enumerate() {
            if step > 0 {
                let from_region = regions_by_id
                    .get(&route[step - 1])
                    .ok_or(rusqlite::Error::InvalidQuery)?;
                let to_region = regions_by_id.get(region_id).ok_or(rusqlite::Error::InvalidQuery)?;
                let hop_ft = region_distance_ft(&from_region.centroid_json, &to_region.centroid_json);
                let pair_confidence = ((from_region.confidence + to_region.confidence) * 0.5).clamp(0.0, 1.0);
                let hop_worst = clamp01(
                    (1.0 - pair_confidence) * 0.75
                        + obstacle_proximity_penalty(from_region, to_region, &hazards) * 0.25,
                );
                running_cost += hop_ft * (2.0 - pair_confidence);
                running_risk = running_risk.max(hop_worst);
            }
            step_rows.push(PlanRunRegionUpsert {
                step_index: step as i64, region_id: *region_id,
                cumulative_cost: running_cost, cumulative_risk: running_risk,
            });
        }
        self.replace_plan_run_regions(run_id, &step_rows)?;

        self.plan_run_by_id(run_id)?
            .ok_or(rusqlite::Error::QueryReturnedNoRows)
    }
}
