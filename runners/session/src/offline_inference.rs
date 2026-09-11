//! Offline graph inference over the abstraction samples a session already holds: one tree
//! fragment per epoch, one discrete belief per variable and a pair of message snapshots per
//! adjacent variable pair.

use anyhow::{anyhow, bail, Context, Result};
use chrono::Utc;
use qualia_session_store::{
    mission_types::{
        AnalysisJobKind, AnalysisJobStatus, DomainKind, ExactnessKind, GraphForm, GraphKind,
        MessageDirection,
    },
    AbstractStateSampleRow, AnalysisJobUpsert, BeliefMatrixUpsert, GraphFragmentUpsert,
    MessageSnapshotUpsert, SessionStore,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, HashMap};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// What the caller wants inferred.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceJobSpec {
    pub session_id: i64,
    pub environment_id: Option<i64>,
    pub window_start_sec: Option<f64>,
    pub window_end_sec: Option<f64>,
    pub graph_kind: GraphKind,
    pub require_exact: bool,
}

/// Identifiers of everything the job wrote.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceJobResult {
    pub job_id: i64,
    pub fragment_ids: Vec<i64>,
    pub belief_matrix_ids: Vec<i64>,
    pub message_snapshot_ids: Vec<i64>,
    pub exact_fragment_count: usize,
    pub approximate_fragment_count: usize,
}

/// A discrete distribution over one abstraction's observed symbols.
#[derive(Debug, Clone)]
struct VariableBelief {
    variable_key: String,
    values: BTreeMap<String, f64>,
    entropy: f64,
    normalization_error: f64,
    max_state_key: String,
    sample_count: usize,
}

/// Run one inference job end to end, recording the job's lifecycle in the store as it goes.
pub fn run_offline_inference(
    store: &SessionStore,
    spec: InferenceJobSpec,
) -> Result<InferenceJobResult> {
    validate_spec(store, &spec)?;
    let mut job = queued_job(&spec)?;
    let job_id = store
        .upsert_analysis_job(&job)
        .context("insert queued analysis job")?;

    job.status = AnalysisJobStatus::Running;
    job.started_at = Some(now_iso8601());
    store
        .upsert_analysis_job(&job)
        .context("update analysis job running")?;

    let outcome = build_fragments(store, job_id, &spec);
    let (status, summary, failure, phase) = match &outcome {
        Ok(result) => (
            AnalysisJobStatus::Complete,
            json!({
                "fragment_count": result.fragment_ids.len(),
                "belief_matrix_count": result.belief_matrix_ids.len(),
                "message_snapshot_count": result.message_snapshot_ids.len(),
                "exact_fragment_count": result.exact_fragment_count,
                "approximate_fragment_count": result.approximate_fragment_count,
            }),
            None,
            "update analysis job complete",
        ),
        Err(error) => (
            AnalysisJobStatus::Failed,
            json!({
                "fragment_count": 0,
                "belief_matrix_count": 0,
                "message_snapshot_count": 0,
                "exact_fragment_count": 0,
                "approximate_fragment_count": 0,
            }),
            Some(json!({ "error": format!("{error:#}") })),
            "update analysis job failed",
        ),
    };
    job.status = status;
    job.completed_at = Some(now_iso8601());
    job.summary_json = serde_json::to_string(&summary)?;
    job.failure_json = failure.map(|value| value.to_string());
    store.upsert_analysis_job(&job).context(phase)?;

    outcome
}

/// Rejects a spec whose session or window cannot describe a job.
fn validate_spec(store: &SessionStore, spec: &InferenceJobSpec) -> Result<()> {
    if spec.session_id <= 0 {
        bail!("session_id must be > 0");
    }
    if let (Some(start), Some(end)) = (spec.window_start_sec, spec.window_end_sec) {
        if start > end {
            bail!("window_start_sec must be <= window_end_sec");
        }
    }
    match store.session_by_id(spec.session_id).context("session lookup")? {
        Some(_) => Ok(()),
        None => bail!("session {} not found", spec.session_id),
    }
}

/// The queued record a new inference job starts from.
fn queued_job(spec: &InferenceJobSpec) -> Result<AnalysisJobUpsert> {
    let requested_at = now_iso8601();
    Ok(AnalysisJobUpsert {
        session_id: spec.session_id,
        environment_id: spec.environment_id,
        job_kind: AnalysisJobKind::GraphInference,
        status: AnalysisJobStatus::Queued,
        requested_at,
        started_at: None,
        completed_at: None,
        window_start_sec: spec.window_start_sec,
        window_end_sec: spec.window_end_sec,
        spec_json: serde_json::to_string(spec)?,
        summary_json: "{}".to_string(),
        failure_json: None,
    })
}

/// One tree fragment per epoch that has samples inside the requested window.
fn build_fragments(
    store: &SessionStore,
    job_id: i64,
    spec: &InferenceJobSpec,
) -> Result<InferenceJobResult> {
    let streams = store
        .list_streams(spec.session_id)
        .context("list streams for inference")?;
    let stream_key = streams
        .iter()
        .find(|stream| stream.role == "observation")
        .map(|stream| stream.stream_key.clone())
        .or_else(|| streams.first().map(|stream| stream.stream_key.clone()))
        .unwrap_or_else(|| "derived".to_string());

    let epochs = store
        .list_epochs(spec.session_id)
        .context("list epochs for inference")?;
    if epochs.is_empty() {
        return Err(anyhow!(
            "session {} has no abstraction epochs",
            spec.session_id
        ));
    }

    let exactness = if spec.require_exact {
        ExactnessKind::Exact
    } else {
        ExactnessKind::Approximate
    };

    let mut fragment_ids = Vec::new();
    let mut belief_matrix_ids = Vec::new();
    let mut message_snapshot_ids = Vec::new();

    for epoch in epochs {
        let spaces = store
            .list_spaces(epoch.id)
            .with_context(|| format!("list spaces for epoch {}", epoch.id))?;
        if spaces.is_empty() {
            continue;
        }

        let mut beliefs = Vec::new();
        for space in spaces {
            let samples = store
                .list_state_samples(space.id, None, None, None)
                .with_context(|| format!("list samples for space {}", space.id))?;
            let windowed = samples
                .into_iter()
                .filter(|sample| {
                    in_window(
                        sample.timestamp_sec,
                        spec.window_start_sec,
                        spec.window_end_sec,
                    )
                })
                .collect::<Vec<_>>();
            if windowed.is_empty() {
                continue;
            }
            beliefs.push(derive_belief(&space.abstraction_name, &windowed)?);
        }
        if beliefs.is_empty() {
            continue;
        }

        let fragment_key = format!("offline_infer:job_{job_id}:epoch_{}", epoch.epoch_index);
        let summary_json = serde_json::to_string(&json!({
            "epoch_id": epoch.id,
            "epoch_index": epoch.epoch_index,
            "variable_keys": beliefs.iter().map(|belief| belief.variable_key.clone()).collect::<Vec<_>>(),
            "sample_counts": beliefs
                .iter()
                .map(|belief| (belief.variable_key.clone(), belief.sample_count))
                .collect::<HashMap<_, _>>(),
            "window_start_sec": spec.window_start_sec,
            "window_end_sec": spec.window_end_sec,
        }))?;

        let fragment_id = store.upsert_graph_fragment(&GraphFragmentUpsert {
            analysis_job_id: job_id,
            session_id: spec.session_id,
            epoch_id: Some(epoch.id),
            stream_key: stream_key.clone(),
            fragment_key,
            graph_kind: spec.graph_kind.clone(),
            graph_form: GraphForm::Tree,
            exactness: exactness.clone(),
            variable_count: beliefs.len() as i64,
            factor_count: beliefs.len().saturating_sub(1) as i64,
            tree_width: Some(1),
            root_variable_key: beliefs.first().map(|belief| belief.variable_key.clone()),
            window_start_sec: spec.window_start_sec.unwrap_or(0.0),
            window_end_sec: spec.window_end_sec.unwrap_or(f64::MAX),
            summary_json,
        })?;
        fragment_ids.push(fragment_id);

        for belief in &beliefs {
            let values_json = serde_json::to_string(&belief.values)?;
            belief_matrix_ids.push(store.upsert_belief_matrix(&BeliefMatrixUpsert {
                fragment_id,
                variable_key: belief.variable_key.clone(),
                domain_kind: DomainKind::Discrete,
                normalization_error: belief.normalization_error,
                entropy: belief.entropy,
                max_state_key: belief.max_state_key.clone(),
                values_json: values_json.clone(),
                matrix_hash: stable_hash(&format!("{}|{}", belief.variable_key, values_json)),
            })?);
        }

        for window in beliefs.windows(2) {
            let left = &window[0];
            let right = &window[1];
            let edge_key = format!("{}--{}", left.variable_key, right.variable_key);
            let left_json = serde_json::to_string(&left.values)?;
            let right_json = serde_json::to_string(&right.values)?;
            let residual = l1_distance(&left.values, &right.values);

            message_snapshot_ids.push(store.upsert_message_snapshot(&MessageSnapshotUpsert {
                fragment_id,
                edge_key: edge_key.clone(),
                direction: MessageDirection::VariableToFactor,
                iteration_index: 0,
                source_node_key: left.variable_key.clone(),
                target_node_key: format!("factor:{edge_key}"),
                values_json: left_json.clone(),
                residual_norm: residual,
                message_hash: stable_hash(&format!("v2f|{edge_key}|{left_json}")),
            })?);
            message_snapshot_ids.push(store.upsert_message_snapshot(&MessageSnapshotUpsert {
                fragment_id,
                edge_key: edge_key.clone(),
                direction: MessageDirection::FactorToVariable,
                iteration_index: 0,
                source_node_key: format!("factor:{edge_key}"),
                target_node_key: right.variable_key.clone(),
                values_json: right_json.clone(),
                residual_norm: residual,
                message_hash: stable_hash(&format!("f2v|{edge_key}|{right_json}")),
            })?);
        }
    }

    if fragment_ids.is_empty() {
        return Err(anyhow!(
            "no graph fragments built from session {} in requested window",
            spec.session_id
        ));
    }

    Ok(InferenceJobResult {
        job_id,
        belief_matrix_ids,
        message_snapshot_ids,
        exact_fragment_count: if spec.require_exact { fragment_ids.len() } else { 0 },
        approximate_fragment_count: if spec.require_exact { 0 } else { fragment_ids.len() },
        fragment_ids,
    })
}

fn in_window(timestamp_sec: f64, start: Option<f64>, end: Option<f64>) -> bool {
    let after_start = start.map_or(true, |start| timestamp_sec >= start);
    let before_end = end.map_or(true, |end| timestamp_sec <= end);
    after_start && before_end
}

/// Symbol frequencies of one abstraction, normalised to a distribution.
fn derive_belief(
    abstraction_name: &str,
    samples: &[AbstractStateSampleRow],
) -> Result<VariableBelief> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for sample in samples {
        *counts.entry(sample.symbol_key.clone()).or_insert(0) += 1;
    }
    let total = counts.values().sum::<usize>();
    if total == 0 {
        bail!("cannot derive belief with empty sample count");
    }

    let total_f = total as f64;
    let mut values = BTreeMap::new();
    let mut entropy = 0.0;
    let mut max_state_key = String::new();
    let mut max_probability = f64::MIN;
    for (state, count) in counts {
        let probability = count as f64 / total_f;
        if probability > 0.0 {
            entropy -= probability * probability.ln();
        }
        if probability > max_probability {
            max_probability = probability;
            max_state_key = state.clone();
        }
        values.insert(state, probability);
    }

    let normalization_error = (1.0 - values.values().sum::<f64>()).abs();
    Ok(VariableBelief {
        variable_key: abstraction_name.to_string(),
        values,
        entropy,
        normalization_error,
        max_state_key,
        sample_count: total,
    })
}

/// Total-variation style L1 distance over the union of both support sets.
fn l1_distance(left: &BTreeMap<String, f64>, right: &BTreeMap<String, f64>) -> f64 {
    left.keys()
        .chain(right.keys().filter(|key| !left.contains_key(*key)))
        .map(|key| {
            let l = left.get(key).copied().unwrap_or(0.0);
            let r = right.get(key).copied().unwrap_or(0.0);
            (l - r).abs()
        })
        .sum()
}

fn stable_hash(text: &str) -> String {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn now_iso8601() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}
