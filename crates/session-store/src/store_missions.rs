//! Mission and coaching persistence: portfolios, instances, phases, outcome
//! assessments, gate and scenario reviews, plus the composite read-side session
//! views and the coach review generator.
//!
//! The scoring in [`SessionStore::generate_coach_scenario_review`] is derived
//! purely from the stored world regions and plan runs; the result is written
//! through the same upsert path callers use.

use crate::mission_types::{
    CoachScenarioReviewRow, GateReviewEntryRow, MissionInstanceRow, MissionPhaseRow,
    MissionPortfolioRow, OutcomeAssessmentRow, PlanRunRow, RegionKind, WorldRegionRow,
};
use crate::rows::{clamp01, clearance_to_obstacles, map_plan_run_row, obstacle_proximity_penalty};
use crate::{
    GateReviewEntryUpsert, MissionInstanceUpsert, MissionPhaseUpsert, MissionPortfolioUpsert,
    OutcomeAssessmentUpsert, SessionEnvironmentViewRow, SessionStore, SessionViewSummaryRow,
};
use rusqlite::{params, OptionalExtension};
use serde_json::Value;
use std::cmp::Ordering;
use std::collections::HashMap;

impl SessionStore {
    pub fn upsert_coach_scenario_review(&self, environment_id: i64, plan_run_id: Option<i64>, generated_at: &str, risk_score: f64, confidence_score: f64, summary_json: &str) -> rusqlite::Result<i64> {
        self.connection.execute(
            r#"
            INSERT INTO coach_scenario_reviews (
                environment_id, plan_run_id, generated_at, risk_score, confidence_score, summary_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(environment_id, plan_run_id, generated_at) DO UPDATE SET
                risk_score=excluded.risk_score,
                confidence_score=excluded.confidence_score,
                summary_json=excluded.summary_json
            "#,
            params![environment_id, plan_run_id, generated_at, clamp01(risk_score), clamp01(confidence_score), summary_json],
        )?;

        let stored_id = self.connection.query_row(
            "SELECT id FROM coach_scenario_reviews WHERE environment_id = ?1 AND plan_run_id IS ?2 AND generated_at = ?3",
            params![environment_id, plan_run_id, generated_at],
            |row| row.get::<_, i64>(0),
        )?;
        Ok(stored_id)
    }

    pub fn list_coach_scenario_reviews(&self, environment_id: i64) -> rusqlite::Result<Vec<CoachScenarioReviewRow>> {
        let mut statement = self.connection.prepare(
            r#"
            SELECT id, environment_id, plan_run_id, generated_at, risk_score, confidence_score, summary_json
            FROM coach_scenario_reviews
            WHERE environment_id = ?1
            ORDER BY generated_at DESC, id DESC
            "#,
        )?;
        let mapped = statement.query_map(params![environment_id], |row| Ok(CoachScenarioReviewRow { id: row.get(0)?, environment_id: row.get(1)?, plan_run_id: row.get(2)?, generated_at: row.get(3)?, risk_score: row.get(4)?, confidence_score: row.get(5)?, summary_json: row.get(6)? }))?;
        mapped.collect()
    }

    pub fn upsert_gate_review_entry(&self, entry: &GateReviewEntryUpsert) -> rusqlite::Result<i64> {
        let gate_number = entry.gate_number.max(1);
        let confidence_score = clamp01(entry.confidence_score);

        if let Some(existing_id) = entry.id {
            self.connection.execute(
                r#"
                UPDATE gate_review_entries
                SET session_id = ?2,
                    gate_number = ?3,
                    gate_label = ?4,
                    track_section = ?5,
                    frame_index = ?6,
                    timestamp_sec = ?7,
                    confidence_score = ?8,
                    pass_order = ?9,
                    status = ?10,
                    source = ?11,
                    note = ?12,
                    summary_json = ?13
                WHERE id = ?1
                "#,
                params![existing_id, entry.session_id, gate_number, entry.gate_label, entry.track_section, entry.frame_index, entry.timestamp_sec, confidence_score, entry.pass_order, entry.status, entry.source, entry.note, entry.summary_json],
            )?;
            return Ok(existing_id);
        }

        self.connection.execute(
            r#"
            INSERT INTO gate_review_entries (
                session_id, gate_number, gate_label, track_section, frame_index, timestamp_sec,
                confidence_score, pass_order, status, source, note, summary_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
            "#,
            params![entry.session_id, gate_number, entry.gate_label, entry.track_section, entry.frame_index, entry.timestamp_sec, confidence_score, entry.pass_order, entry.status, entry.source, entry.note, entry.summary_json],
        )?;
        Ok(self.connection.last_insert_rowid())
    }

    pub fn list_gate_review_entries(&self, session_id: i64) -> rusqlite::Result<Vec<GateReviewEntryRow>> {
        let mut statement = self.connection.prepare(
            r#"
            SELECT id, session_id, gate_number, gate_label, track_section, frame_index, timestamp_sec,
                   confidence_score, pass_order, status, source, note, summary_json
            FROM gate_review_entries
            WHERE session_id = ?1
            ORDER BY COALESCE(pass_order, 9223372036854775807) ASC,
                     gate_number ASC,
                     COALESCE(timestamp_sec, 1.0e18) ASC,
                     id ASC
            "#,
        )?;
        let mapped = statement.query_map(params![session_id], |row| Ok(GateReviewEntryRow { id: row.get(0)?, session_id: row.get(1)?, gate_number: row.get(2)?, gate_label: row.get(3)?, track_section: row.get(4)?, frame_index: row.get(5)?, timestamp_sec: row.get(6)?, confidence_score: row.get(7)?, pass_order: row.get(8)?, status: row.get(9)?, source: row.get(10)?, note: row.get(11)?, summary_json: row.get(12)? }))?;
        mapped.collect()
    }

    pub fn upsert_mission_portfolio(&self, portfolio: &MissionPortfolioUpsert) -> rusqlite::Result<i64> {
        self.connection.execute(
            r#"
            INSERT INTO mission_portfolios (
                portfolio_key, mission_type, mission_subtype, display_name, description,
                rubric_json, metadata_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(portfolio_key) DO UPDATE SET
                mission_type=excluded.mission_type,
                mission_subtype=excluded.mission_subtype,
                display_name=excluded.display_name,
                description=excluded.description,
                rubric_json=excluded.rubric_json,
                metadata_json=excluded.metadata_json
            "#,
            params![portfolio.portfolio_key, portfolio.mission_type, portfolio.mission_subtype, portfolio.display_name, portfolio.description, portfolio.rubric_json, portfolio.metadata_json],
        )?;

        let stored_id = self.connection.query_row(
            "SELECT id FROM mission_portfolios WHERE portfolio_key = ?1",
            params![portfolio.portfolio_key],
            |row| row.get::<_, i64>(0),
        )?;
        Ok(stored_id)
    }

    pub fn list_mission_portfolios(&self) -> rusqlite::Result<Vec<MissionPortfolioRow>> {
        let mut statement = self.connection.prepare(
            r#"
            SELECT id, portfolio_key, mission_type, mission_subtype, display_name, description,
                   rubric_json, metadata_json
            FROM mission_portfolios
            ORDER BY mission_type ASC, mission_subtype ASC, display_name ASC, id ASC
            "#,
        )?;
        let mapped = statement.query_map([], |row| Ok(MissionPortfolioRow { id: row.get(0)?, portfolio_key: row.get(1)?, mission_type: row.get(2)?, mission_subtype: row.get(3)?, display_name: row.get(4)?, description: row.get(5)?, rubric_json: row.get(6)?, metadata_json: row.get(7)? }))?;
        mapped.collect()
    }

    pub fn upsert_mission_instance(&self, instance: &MissionInstanceUpsert) -> rusqlite::Result<i64> {
        self.connection.execute(
            r#"
            INSERT INTO mission_instances (
                portfolio_id, session_id, instance_key, title, status, started_at,
                completed_at, objective_summary, summary_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            ON CONFLICT(portfolio_id, instance_key) DO UPDATE SET
                session_id=excluded.session_id,
                title=excluded.title,
                status=excluded.status,
                started_at=excluded.started_at,
                completed_at=excluded.completed_at,
                objective_summary=excluded.objective_summary,
                summary_json=excluded.summary_json
            "#,
            params![instance.portfolio_id, instance.session_id, instance.instance_key, instance.title, instance.status, instance.started_at, instance.completed_at, instance.objective_summary, instance.summary_json],
        )?;

        let stored_id = self.connection.query_row(
            "SELECT id FROM mission_instances WHERE portfolio_id = ?1 AND instance_key = ?2",
            params![instance.portfolio_id, instance.instance_key],
            |row| row.get::<_, i64>(0),
        )?;
        Ok(stored_id)
    }

    pub fn list_mission_instances(&self, portfolio_id: i64) -> rusqlite::Result<Vec<MissionInstanceRow>> {
        let mut statement = self.connection.prepare(
            r#"
            SELECT id, portfolio_id, session_id, instance_key, title, status, started_at,
                   completed_at, objective_summary, summary_json
            FROM mission_instances
            WHERE portfolio_id = ?1
            ORDER BY started_at ASC, id ASC
            "#,
        )?;
        let mapped = statement.query_map(params![portfolio_id], |row| Ok(MissionInstanceRow { id: row.get(0)?, portfolio_id: row.get(1)?, session_id: row.get(2)?, instance_key: row.get(3)?, title: row.get(4)?, status: row.get(5)?, started_at: row.get(6)?, completed_at: row.get(7)?, objective_summary: row.get(8)?, summary_json: row.get(9)? }))?;
        mapped.collect()
    }

    pub fn upsert_mission_phase(&self, phase: &MissionPhaseUpsert) -> rusqlite::Result<i64> {
        self.connection.execute(
            r#"
            INSERT INTO mission_phases (
                mission_instance_id, phase_key, phase_kind, window_start_sec, window_end_sec,
                status, summary_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(mission_instance_id, phase_key) DO UPDATE SET
                phase_kind=excluded.phase_kind,
                window_start_sec=excluded.window_start_sec,
                window_end_sec=excluded.window_end_sec,
                status=excluded.status,
                summary_json=excluded.summary_json
            "#,
            params![phase.mission_instance_id, phase.phase_key, phase.phase_kind, phase.window_start_sec, phase.window_end_sec, phase.status, phase.summary_json],
        )?;

        let stored_id = self.connection.query_row(
            "SELECT id FROM mission_phases WHERE mission_instance_id = ?1 AND phase_key = ?2",
            params![phase.mission_instance_id, phase.phase_key],
            |row| row.get::<_, i64>(0),
        )?;
        Ok(stored_id)
    }

    pub fn list_mission_phases(&self, mission_instance_id: i64) -> rusqlite::Result<Vec<MissionPhaseRow>> {
        let mut statement = self.connection.prepare(
            r#"
            SELECT id, mission_instance_id, phase_key, phase_kind, window_start_sec, window_end_sec,
                   status, summary_json
            FROM mission_phases
            WHERE mission_instance_id = ?1
            ORDER BY window_start_sec ASC, id ASC
            "#,
        )?;
        let mapped = statement.query_map(params![mission_instance_id], |row| Ok(MissionPhaseRow { id: row.get(0)?, mission_instance_id: row.get(1)?, phase_key: row.get(2)?, phase_kind: row.get(3)?, window_start_sec: row.get(4)?, window_end_sec: row.get(5)?, status: row.get(6)?, summary_json: row.get(7)? }))?;
        mapped.collect()
    }

    pub fn upsert_outcome_assessment(&self, assessment: &OutcomeAssessmentUpsert) -> rusqlite::Result<i64> {
        self.connection.execute(
            r#"
            INSERT INTO outcome_assessments (
                mission_instance_id, phase_id, assessment_status, overall_result, scorecard_json,
                evidence_json, recommended_plan_revision, assessor, assessed_at, summary_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            ON CONFLICT(mission_instance_id, phase_id, assessor, assessed_at) DO UPDATE SET
                assessment_status=excluded.assessment_status,
                overall_result=excluded.overall_result,
                scorecard_json=excluded.scorecard_json,
                evidence_json=excluded.evidence_json,
                recommended_plan_revision=excluded.recommended_plan_revision,
                summary_json=excluded.summary_json
            "#,
            params![assessment.mission_instance_id, assessment.phase_id, assessment.assessment_status, assessment.overall_result, assessment.scorecard_json, assessment.evidence_json, assessment.recommended_plan_revision, assessment.assessor, assessment.assessed_at, assessment.summary_json],
        )?;

        let stored_id = self.connection.query_row(
            r#"
            SELECT id FROM outcome_assessments
            WHERE mission_instance_id = ?1 AND phase_id IS ?2 AND assessor = ?3 AND assessed_at = ?4
            "#,
            params![assessment.mission_instance_id, assessment.phase_id, assessment.assessor, assessment.assessed_at],
            |row| row.get::<_, i64>(0),
        )?;
        Ok(stored_id)
    }

    pub fn list_outcome_assessments(&self, mission_instance_id: i64) -> rusqlite::Result<Vec<OutcomeAssessmentRow>> {
        let mut statement = self.connection.prepare(
            r#"
            SELECT id, mission_instance_id, phase_id, assessment_status, overall_result,
                   scorecard_json, evidence_json, recommended_plan_revision, assessor,
                   assessed_at, summary_json
            FROM outcome_assessments
            WHERE mission_instance_id = ?1
            ORDER BY assessed_at DESC, id DESC
            "#,
        )?;
        let mapped = statement.query_map(params![mission_instance_id], |row| Ok(OutcomeAssessmentRow { id: row.get(0)?, mission_instance_id: row.get(1)?, phase_id: row.get(2)?, assessment_status: row.get(3)?, overall_result: row.get(4)?, scorecard_json: row.get(5)?, evidence_json: row.get(6)?, recommended_plan_revision: row.get(7)?, assessor: row.get(8)?, assessed_at: row.get(9)?, summary_json: row.get(10)? }))?;
        mapped.collect()
    }

    /// Scores one environment's stored regions and plan runs. One review is
    /// written per plan run; when no plan exists a single plan-less review is
    /// written instead.
    pub fn generate_coach_scenario_review(&self, environment_id: i64, generated_at: &str) -> rusqlite::Result<Vec<CoachScenarioReviewRow>> {
        let environment_regions = self.list_world_regions_for_environment(environment_id)?;
        let plan_runs = self.list_plan_runs(environment_id)?;

        let obstacle_regions: Vec<WorldRegionRow> = environment_regions.iter().filter(|r| r.region_kind == RegionKind::ObstacleCluster).cloned().collect();
        let region_index: HashMap<i64, WorldRegionRow> = environment_regions.iter().map(|r| (r.id, r.clone())).collect();
        let mean_confidence = if environment_regions.is_empty() { 0.0 } else { environment_regions.iter().map(|r| r.confidence).sum::<f64>() / environment_regions.len() as f64 };

        let mut fragile: Vec<Value> = environment_regions.iter().filter(|r| r.confidence < 0.65).map(|r| serde_json::json!({"region_id": r.id, "region_key": r.region_key, "confidence": r.confidence})).collect();
        let reported_confidence =
            |sample: &Value| sample.get("confidence").and_then(Value::as_f64).unwrap_or(1.0);
        fragile.sort_by(|a, b| reported_confidence(a).partial_cmp(&reported_confidence(b)).unwrap_or(Ordering::Equal));
        fragile.truncate(8);

        let mut written = Vec::new();
        for plan in &plan_runs {
            let steps = self.list_plan_run_regions(plan.id)?;
            let mut narrow = Vec::new();
            let mut risky = Vec::new();

            for step in 0..steps.len().saturating_sub(1) {
                let head = region_index.get(&steps[step].region_id);
                let tail = region_index.get(&steps[step + 1].region_id);
                let (Some(left), Some(right)) = (head, tail) else {
                    continue;
                };

                let headroom = clearance_to_obstacles(left, &obstacle_regions).min(clearance_to_obstacles(right, &obstacle_regions));
                if headroom > 0.0 && headroom < 2.0 {
                    narrow.push(serde_json::json!({"from_region_id": left.id, "to_region_id": right.id, "clearance_ft": headroom}));
                }

                let hop_risk = clamp01((1.0 - ((left.confidence + right.confidence) * 0.5)) * 0.75 + obstacle_proximity_penalty(left, right, &obstacle_regions) * 0.25);
                if hop_risk >= 0.60 {
                    risky.push(serde_json::json!({"from_region_id": left.id, "to_region_id": right.id, "risk": hop_risk}));
                }
            }

            let mut advice: Vec<String> = Vec::new();
            if !fragile.is_empty() {
                advice.push("Re-check the low-confidence regions on a second pass before committing.".to_string());
            }
            if !narrow.is_empty() {
                advice.push("Widen the corridor where clearance is tight, or slow the approach.".to_string());
            }
            if !risky.is_empty() {
                advice.push("Add waypoints so the high-risk links are crossed in shorter hops.".to_string());
            }
            if advice.is_empty() {
                advice.push("Nothing stands out in this run; keep perturbation coverage (lighting and pose drift) before release.".to_string());
            }
            advice.truncate(4);

            let step_total = steps.len().max(1) as f64;
            let review_risk = clamp01((plan.risk_score * 0.55) + ((narrow.len() as f64 / step_total) * 0.25) + ((risky.len() as f64 / step_total) * 0.20));
            let confidence_score = clamp01((mean_confidence * 0.6) + ((1.0 - review_risk) * 0.4));

            let summary = serde_json::json!({
                "signals": {"low_confidence_regions": fragile, "narrow_clearance_segments": narrow, "high_risk_links": risky},
                "suggestions": advice,
                "totals": {"region_count": environment_regions.len(), "plan_step_count": steps.len()}
            })
            .to_string();

            let id = self.upsert_coach_scenario_review(environment_id, Some(plan.id), generated_at, review_risk, confidence_score, &summary)?;
            written.push(CoachScenarioReviewRow { id, environment_id, plan_run_id: Some(plan.id), generated_at: generated_at.to_string(), risk_score: review_risk, confidence_score, summary_json: summary });
        }

        if written.is_empty() {
            let summary = serde_json::json!({
                "signals": {"low_confidence_regions": fragile, "narrow_clearance_segments": [], "high_risk_links": []},
                "suggestions": ["No plan runs are stored yet; write one to get route-level scenario shaping."],
                "totals": {"region_count": environment_regions.len(), "plan_step_count": 0}
            })
            .to_string();
            let risk = clamp01((1.0 - mean_confidence).max(0.0));
            let confidence = clamp01(mean_confidence);
            let id = self.upsert_coach_scenario_review(environment_id, None, generated_at, risk, confidence, &summary)?;
            written.push(CoachScenarioReviewRow { id, environment_id, plan_run_id: None, generated_at: generated_at.to_string(), risk_score: risk, confidence_score: confidence, summary_json: summary });
        }

        Ok(written)
    }

    pub fn plan_run_by_id(&self, plan_run_id: i64) -> rusqlite::Result<Option<PlanRunRow>> {
        let found = self.connection.query_row(
            r#"
            SELECT id, environment_id, session_id, source_region_id, target_region_id,
                   planner_kind, status, path_cost, risk_score, clearance_min_ft,
                   started_at, completed_at, summary_json
            FROM plan_runs
            WHERE id = ?1
            "#,
            params![plan_run_id],
            map_plan_run_row,
        ).optional()?;
        Ok(found)
    }

    pub fn session_view_summary(&self, session_id: i64) -> rusqlite::Result<SessionViewSummaryRow> {
        let summary = self.connection.query_row(
            r#"
            SELECT ?1,
                   (SELECT COUNT(*) FROM analysis_jobs WHERE session_id = ?1),
                   (SELECT COUNT(*) FROM graph_fragments WHERE session_id = ?1),
                   (SELECT COUNT(*) FROM world_regions WHERE session_id = ?1),
                   (SELECT COUNT(*) FROM plan_runs WHERE session_id = ?1)
            "#,
            params![session_id],
            |row| Ok(SessionViewSummaryRow { session_id: row.get(0)?, analysis_jobs: row.get(1)?, graph_fragments: row.get(2)?, world_regions: row.get(3)?, plan_runs: row.get(4)? }),
        )?;
        Ok(summary)
    }

    pub fn session_environment_view(&self, session_id: i64) -> rusqlite::Result<SessionEnvironmentViewRow> {
        let session_summary = self.session_view_summary(session_id)?;
        let world_regions = self.list_world_regions(session_id)?;
        let plan_runs = self.list_plan_runs_for_session(session_id)?;
        Ok(SessionEnvironmentViewRow { session_summary, world_regions, plan_runs })
    }
}
