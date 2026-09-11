//! Train the JEPA candidate and write its immutable evidence.
//!
//! The binary is a pipeline: preflight the dataset gate, materialize every
//! sealed session, train the CNN candidate and the flat baseline on the
//! training split, calibrate the no-change baseline, measure both held-out
//! splits, compute the effective rank, and only then publish the training
//! report and the candidate checkpoint. Nothing is published before the report
//! exists, and the report is addressed by its own digest.

use qualia_jepa_dataset::manifest_digest;
use qualia_jepa_dataset::materialize_session;
use qualia_jepa_dataset::validate_dataset_promotion_gate;
use qualia_jepa_dataset::DatasetManifest;
use qualia_jepa_dataset::DatasetSplit;
use qualia_jepa_model::device_for_backend;
use qualia_jepa_model::empty_candidate_manifest;
use qualia_jepa_model::evaluation::effective_rank_report;
use qualia_jepa_model::evaluation::CalibrationAccumulator;
use qualia_jepa_model::evaluation::OccupancyAccumulator;
use qualia_jepa_model::measured_action_support;
use qualia_jepa_model::split_grounding_calibration_gate_passes;
use qualia_jepa_model::split_predictive_gate_passes;
use qualia_jepa_model::train::FlatOfflineTrainer;
use qualia_jepa_model::train::OfflineTrainer;
use qualia_jepa_model::train::PredictionBatch;
use qualia_jepa_model::train::StepMetrics;
use qualia_jepa_model::train::TrainerConfig;
use qualia_jepa_model::train::TransitionExample;
use qualia_jepa_model::write_candidate_checkpoint;
use qualia_jepa_model::BaselineGate;
use qualia_jepa_model::GroundingGeometry;
use qualia_jepa_model::HeldOutMetrics;
use qualia_jepa_model::SplitEvaluation;
use qualia_jepa_model::TrainingReport;
use qualia_jepa_model::TRAINING_REPORT_SCHEMA;
use sha2::Digest;
use sha2::Sha256;
use std::collections::BTreeMap;
use std::error::Error;
use std::ffi::OsString;
use std::fs;
use std::fs::OpenOptions;
use std::path::Path;
use std::path::PathBuf;
use std::time::Instant;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

type CliResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

const USAGE: &str = concat!(
    "usage: qualia-jepa-train --manifest <dataset.json> --checkpoint-id <id> ",
    "[--output-dir <dir>] [--backend cpu|metal|cuda] [--epochs N] ",
    "[--batch-size N] [--seed N]",
);

#[derive(Debug)]
struct Args {
    manifest_path: PathBuf,
    output_dir: PathBuf,
    checkpoint_name: String,
    backend_name: String,
    epoch_count: usize,
    batch_size: usize,
    seed: u64,
}

/// Sample-weighted accumulator for one held-out split.
#[derive(Default)]
struct MetricAccumulator {
    seen_samples: u64,
    seen_sessions: u64,
    nochange_nll: f64,
    nochange_rollout: f64,
    baseline_nll: f64,
    baseline_rollout: f64,
    candidate_nll: f64,
    candidate_rollout: f64,
    calibration_stats: CalibrationAccumulator,
    occupancy_stats: OccupancyAccumulator,
}

/// Sample-weighted accumulator for one training epoch.
#[derive(Default)]
struct TrainingEpochAccumulator {
    seen_samples: u64,
    seen_batches: u64,
    candidate_total: f64,
    candidate_nll: f64,
    candidate_variance: f64,
    candidate_covariance: f64,
    candidate_grounding: f64,
    baseline_total: f64,
    baseline_nll: f64,
}

impl TrainingEpochAccumulator {
    fn push(&mut self, sample_count: usize, candidate: StepMetrics, baseline: StepMetrics) {
        let weight = sample_count as f64;
        self.seen_samples += sample_count as u64;
        self.seen_batches += 1;
        let totals = [
            (&mut self.candidate_total, f64::from(candidate.total)),
            (&mut self.candidate_nll, f64::from(candidate.transition_nll)),
            (&mut self.candidate_variance, f64::from(candidate.variance)),
            (
                &mut self.candidate_covariance,
                f64::from(candidate.covariance),
            ),
            (
                &mut self.candidate_grounding,
                f64::from(candidate.grounding),
            ),
            (&mut self.baseline_total, f64::from(baseline.total)),
            (&mut self.baseline_nll, f64::from(baseline.transition_nll)),
        ];
        for (slot, value) in totals {
            *slot += value * weight;
        }
    }

    fn mean(&self, total: f64) -> f64 {
        match self.seen_samples {
            0 => 0.0,
            count => total / count as f64,
        }
    }
}

/// Render the per-epoch stderr progress line.
///
/// The wording is emitted interface (docs/decisions.md D-008/D-009), so it is
/// produced by a pure function that a test can pin without running the
/// pipeline — the promotion gate this binary enforces needs 4096 held-out
/// transitions per split before training starts.
fn epoch_log_line(
    epoch: usize,
    epochs: usize,
    backend: &str,
    stats: &TrainingEpochAccumulator,
    seconds: f64,
    samples_per_second: f64,
) -> String {
    format!(
        "jepa train epoch={}/{} backend={} samples={} batches={} cnn_total={:.6} cnn_nll={:.6} cnn_var={:.6} cnn_cov={:.6} cnn_ground={:.6} flat_total={:.6} flat_nll={:.6} seconds={:.3} samples_per_second={:.2}",
        epoch,
        epochs,
        backend,
        stats.seen_samples,
        stats.seen_batches,
        stats.mean(stats.candidate_total),
        stats.mean(stats.candidate_nll),
        stats.mean(stats.candidate_variance),
        stats.mean(stats.candidate_covariance),
        stats.mean(stats.candidate_grounding),
        stats.mean(stats.baseline_total),
        stats.mean(stats.baseline_nll),
        seconds,
        samples_per_second
    )
}

/// Render the constant-baseline calibration stderr line.
fn calibration_log_line(samples: usize) -> String {
    format!("jepa calibrate constant_baseline_samples={}", samples)
}

impl MetricAccumulator {
    fn push(
        &mut self,
        sample_count: usize,
        constant: &HeldOutMetrics,
        baseline: &HeldOutMetrics,
        candidate: &HeldOutMetrics,
    ) {
        let weight = sample_count as f64;
        self.seen_samples += sample_count as u64;
        self.seen_sessions += 1;
        let deltas = [
            (&mut self.nochange_nll, constant.transition_nll),
            (&mut self.nochange_rollout, constant.rollout_error),
            (&mut self.baseline_nll, baseline.transition_nll),
            (&mut self.baseline_rollout, baseline.rollout_error),
            (&mut self.candidate_nll, candidate.transition_nll),
            (&mut self.candidate_rollout, candidate.rollout_error),
        ];
        for (slot, value) in deltas {
            *slot += value * weight;
        }
    }

    fn finish(self) -> CliResult<SplitEvaluation> {
        if self.seen_samples == 0 || self.seen_sessions == 0 {
            return Err("held-out split has no materialized samples".into());
        }
        let divisor = self.seen_samples as f64;
        let combine = |nll: f64, rollout: f64| HeldOutMetrics {
            transition_nll: nll / divisor,
            rollout_error: rollout / divisor,
        };
        Ok(SplitEvaluation {
            samples: self.seen_samples,
            sessions: self.seen_sessions,
            constant: combine(self.nochange_nll, self.nochange_rollout),
            flat_mlp: combine(self.baseline_nll, self.baseline_rollout),
            tiny_cnn: combine(self.candidate_nll, self.candidate_rollout),
            calibration: self.calibration_stats.finish()?,
            occupancy: self.occupancy_stats.finish()?,
        })
    }

    fn push_predictions(
        &mut self,
        examples: &[TransitionExample],
        predictions: &PredictionBatch,
    ) -> CliResult<()> {
        let expected = examples.len();
        let lengths = [
            predictions.predicted_mean.len(),
            predictions.predicted_log_variance.len(),
            predictions.target_latent.len(),
            predictions.occupancy_logits.len(),
        ];
        if lengths.into_iter().any(|actual| actual != expected) {
            return Err("prediction batch does not match held-out examples".into());
        }
        for (position, example) in examples.iter().enumerate() {
            self.calibration_stats.push(
                &predictions.predicted_mean[position],
                &predictions.predicted_log_variance[position],
                &predictions.target_latent[position],
            )?;
            self.occupancy_stats.push(
                &predictions.occupancy_logits[position],
                &example.future_occupied,
                &example.future_observed,
            )?;
        }
        Ok(())
    }
}

fn main() {
    match run() {
        Ok(()) => {}
        Err(error) => {
            eprintln!("qualia-jepa-train: {error}");
            std::process::exit(1);
        }
    }
}

fn run() -> CliResult<()> {
    let args = parse_args()?;
    let manifest: DatasetManifest = serde_json::from_slice(&fs::read(&args.manifest_path)?)?;
    preflight(&manifest, &args)?;
    let device = device_for_backend(&args.backend_name)?;
    let config = TrainerConfig::default();
    let mut candidate = OfflineTrainer::new(device.clone(), args.seed, config)?;
    let mut baseline = FlatOfflineTrainer::new(device, args.seed ^ 0xf1a7_5eed, config)?;
    let (mut training_examples, held_out_sessions) = gather_examples(&manifest)?;
    if training_examples.len() < 2 {
        return Err("training split has fewer than two materialized transitions".into());
    }
    let skipped_singletons =
        train_epochs(&mut candidate, &mut baseline, &mut training_examples, &args)?;
    eprintln!("{}", calibration_log_line(training_examples.len()));
    candidate.calibrate_constant_baseline(&training_examples)?;
    let (validation, test, held_out_representations) =
        measure_holdout(&mut candidate, &mut baseline, &held_out_sessions)?;
    let action_support = measured_action_support(&manifest)?;
    let grounding_geometry = GroundingGeometry {
        width: qualia_jepa::GROUNDING_WIDTH,
        height: qualia_jepa::GROUNDING_HEIGHT,
        resolution_m: manifest.config.grounding_resolution_m,
    };
    let effective_rank = effective_rank_report(&held_out_representations)?;
    let baseline_gate = BaselineGate {
        dataset_digest: manifest.digest.clone(),
        valid_transitions: manifest.audit.valid_transitions,
        sessions: manifest.audit.sessions,
        conditions: manifest.audit.conditions.len() as u64,
        environments: manifest.audit.environments,
        constant: test.constant.clone(),
        flat_mlp: test.flat_mlp.clone(),
        tiny_cnn: test.tiny_cnn.clone(),
    };
    let baseline_gate_passed = baseline_gate.passes()
        && split_predictive_gate_passes(&validation)
        && split_predictive_gate_passes(&test);
    let grounding_calibration_gate_passed = split_grounding_calibration_gate_passes(&validation)
        && split_grounding_calibration_gate_passes(&test);
    let all_gates_passed =
        baseline_gate_passed && grounding_calibration_gate_passed && effective_rank.passes();
    let created_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let report = TrainingReport {
        schema_version: TRAINING_REPORT_SCHEMA.to_string(),
        created_at_ms,
        checkpoint_id: args.checkpoint_name.clone(),
        dataset_digest: manifest.digest.clone(),
        backend: args.backend_name.clone(),
        seed: args.seed,
        epochs: args.epoch_count,
        batch_size: args.batch_size,
        cnn_steps: candidate.steps(),
        flat_steps: baseline.steps(),
        skipped_singletons,
        action_support: action_support.clone(),
        grounding_geometry: grounding_geometry.clone(),
        validation,
        test,
        effective_rank,
        llm_priors_ablated: true,
        baseline_gate_passed,
        grounding_calibration_gate_passed,
        all_gates_passed,
    };
    let (report_path, report_digest) = write_immutable_report(&args.output_dir, &report)?;
    let mut checkpoint = empty_candidate_manifest(
        &args.checkpoint_name,
        &manifest.digest,
        args.seed,
        &args.backend_name,
        baseline_gate,
        action_support,
        grounding_geometry,
    );
    checkpoint.training_report_path = fs::canonicalize(&report_path)?.display().to_string();
    checkpoint.training_report_sha256 = report_digest;
    let (weights, metadata) = write_candidate_checkpoint(
        &args.output_dir,
        &args.checkpoint_name,
        candidate.online_vars(),
        candidate.target_vars(),
        checkpoint,
    )?;
    println!(
        "candidate={} weights={} manifest={} report={} gate={}",
        args.checkpoint_name,
        weights.display(),
        metadata.display(),
        report_path.display(),
        report.all_gates_passed,
    );
    if report.all_gates_passed {
        Ok(())
    } else {
        Err("candidate failed held-out predictive, grounding, or calibration gates".into())
    }
}

fn preflight(dataset: &DatasetManifest, options: &Args) -> CliResult<()> {
    if dataset.schema_version != "qualia.jepa-dataset.v3"
        || dataset.digest != manifest_digest(dataset)?
    {
        return Err("dataset manifest is unsupported or has an invalid digest".into());
    }
    validate_dataset_promotion_gate(dataset)?;
    let split_size =
        |split: DatasetSplit| dataset.audit.split_samples.get(&split).copied().unwrap_or(0);
    let held_out = split_size(DatasetSplit::Validation) + split_size(DatasetSplit::Test);
    if held_out < 4_096 {
        return Err("effective-rank gate requires at least 4096 held-out samples".into());
    }
    if options.epoch_count == 0 || options.batch_size < 2 {
        return Err("epochs must be positive and batch size must be at least two".into());
    }
    Ok(())
}

/// Write the report under its own digest, never overwriting a previous one.
fn write_immutable_report(output: &Path, report: &TrainingReport) -> CliResult<(PathBuf, String)> {
    fs::create_dir_all(output)?;
    let payload = serde_json::to_vec_pretty(report)?;
    let digest = format!("{:x}", Sha256::digest(&payload));
    let target = output.join(format!("jepa-training-report-{digest}.json"));
    if target.exists() {
        if fs::read(&target)? == payload {
            return Ok((target, digest));
        }
        return Err("immutable training report collision".into());
    }
    let staged = target.with_extension("json.partial");
    fs::write(&staged, &payload)?;
    OpenOptions::new().write(true).open(&staged)?.sync_all()?;
    fs::rename(&staged, &target)?;
    #[cfg(unix)]
    OpenOptions::new().read(true).open(output)?.sync_all()?;
    Ok((target, digest))
}

/// Per-epoch shuffle key: seed and epoch first, then the sample identity.
fn epoch_shuffle_key(seed: u64, epoch: usize, example: &TransitionExample) -> [u8; 32] {
    let seed_bytes = seed.to_le_bytes();
    let epoch_bytes = epoch.to_le_bytes();
    let mut hasher = Sha256::new();
    let parts = [
        seed_bytes.as_slice(),
        epoch_bytes.as_slice(),
        example.sample_id.as_bytes(),
    ];
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize().into()
}

/// Materialize every sealed session and partition it into the training pool and
/// the ordered held-out groups.
fn gather_examples(
    manifest: &DatasetManifest,
) -> CliResult<(Vec<TransitionExample>, Vec<(DatasetSplit, Vec<TransitionExample>)>)> {
    let split_of = manifest
        .samples
        .iter()
        .map(|sample| (sample.sample_id.as_str(), sample.split.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut training = Vec::new();
    let mut held_out = Vec::<(DatasetSplit, Vec<TransitionExample>)>::new();
    for (ordinal, source) in manifest.sources.iter().enumerate() {
        eprintln!(
            "jepa materialize source={}/{} session={}",
            ordinal + 1,
            manifest.sources.len(),
            source.session_id
        );
        let mut buckets = [Vec::new(), Vec::new(), Vec::new()];
        for transition in materialize_session(manifest, &source.session_id)? {
            let split = split_of
                .get(transition.sample_id.as_str())
                .ok_or("materialized transition is absent from the dataset split map")?;
            let bucket = match split {
                DatasetSplit::Train => 0,
                DatasetSplit::Validation => 1,
                DatasetSplit::Test => 2,
            };
            buckets[bucket].push(TransitionExample::try_from(&transition)?);
        }
        let [train, validation, test] = buckets;
        eprintln!(
            "jepa materialized session={} train={} validation={} test={}",
            source.session_id,
            train.len(),
            validation.len(),
            test.len()
        );
        training.extend(train);
        if !validation.is_empty() {
            held_out.push((DatasetSplit::Validation, validation));
        }
        if !test.is_empty() {
            held_out.push((DatasetSplit::Test, test));
        }
    }
    Ok((training, held_out))
}

/// Run the seeded epoch schedule and return the singleton batches that were
/// skipped along the way.
fn train_epochs(
    candidate: &mut OfflineTrainer,
    baseline: &mut FlatOfflineTrainer,
    examples: &mut Vec<TransitionExample>,
    args: &Args,
) -> CliResult<u64> {
    let mut skipped = 0u64;
    for epoch in 0..args.epoch_count {
        let started = Instant::now();
        let mut stats = TrainingEpochAccumulator::default();
        examples.sort_by_cached_key(|example| epoch_shuffle_key(args.seed, epoch, example));
        for batch in examples.chunks(args.batch_size) {
            if batch.len() < 2 {
                skipped += batch.len() as u64;
                continue;
            }
            let candidate_step = candidate.train_batch(batch)?;
            let baseline_step = baseline.train_batch(batch)?;
            stats.push(batch.len(), candidate_step, baseline_step);
        }
        let seconds = started.elapsed().as_secs_f64();
        let samples_per_second = if seconds > 0.0 {
            stats.seen_samples as f64 / seconds
        } else {
            0.0
        };
        eprintln!(
            "{}",
            epoch_log_line(
                epoch + 1,
                args.epoch_count,
                &args.backend_name,
                &stats,
                seconds,
                samples_per_second,
            )
        );
    }
    Ok(skipped)
}

/// Evaluate every held-out group with all three predictors and retain up to
/// 4096 target representations for the effective-rank report.
fn measure_holdout(
    candidate: &mut OfflineTrainer,
    baseline: &mut FlatOfflineTrainer,
    groups: &[(DatasetSplit, Vec<TransitionExample>)],
) -> CliResult<(SplitEvaluation, SplitEvaluation, Vec<Vec<f32>>)> {
    let mut validation = MetricAccumulator::default();
    let mut test = MetricAccumulator::default();
    let mut representations = Vec::new();
    for (split, examples) in groups {
        let constant = candidate.constant_sequence(examples)?;
        let candidate_metrics = candidate.evaluate_sequence(examples)?;
        let baseline_metrics = baseline.evaluate_sequence(examples)?;
        let accumulator = match split {
            DatasetSplit::Validation => &mut validation,
            DatasetSplit::Test => &mut test,
            DatasetSplit::Train => unreachable!(),
        };
        accumulator.push(
            examples.len(),
            &constant,
            &baseline_metrics,
            &candidate_metrics,
        );
        for chunk in examples.chunks(64) {
            let predictions = candidate.predict_batch(chunk)?;
            accumulator.push_predictions(chunk, &predictions)?;
            let room = 4_096usize.saturating_sub(representations.len());
            representations.extend(predictions.target_latent.into_iter().take(room));
        }
    }
    Ok((validation.finish()?, test.finish()?, representations))
}

fn parse_args() -> CliResult<Args> {
    let mut manifest = None;
    let mut output = PathBuf::from("artifacts/jepa/checkpoints");
    let mut checkpoint_id = None;
    let mut backend = "cpu".to_string();
    let mut epochs = 20usize;
    let mut batch_size = 32usize;
    let mut seed = 42u64;
    let mut argv = std::env::args_os().skip(1);
    while let Some(token) = argv.next() {
        match token.to_str() {
            Some("--manifest") => manifest = argv.next().map(PathBuf::from),
            Some("--output-dir") => output = take_path(&mut argv, "--output-dir", "a path")?,
            Some("--checkpoint-id") => {
                checkpoint_id = argv.next().and_then(|value| value.into_string().ok())
            }
            Some("--backend") => {
                backend = take_string(&mut argv, "--backend", "cpu, metal, or cuda")?
            }
            Some("--epochs") => epochs = parse_next(&mut argv, "--epochs")?,
            Some("--batch-size") => batch_size = parse_next(&mut argv, "--batch-size")?,
            Some("--seed") => seed = parse_next(&mut argv, "--seed")?,
            Some("--help") | Some("-h") => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            _ => return Err("unknown or incomplete training argument".into()),
        }
    }
    let manifest = match manifest {
        Some(path) => path,
        None => return Err("--manifest is required".into()),
    };
    let checkpoint_id = match checkpoint_id {
        Some(id) => id,
        None => return Err("--checkpoint-id is required".into()),
    };
    Ok(Args {
        manifest_path: manifest,
        output_dir: output,
        checkpoint_name: checkpoint_id,
        backend_name: backend,
        epoch_count: epochs,
        batch_size,
        seed,
    })
}

fn take_path(
    argv: &mut impl Iterator<Item = OsString>,
    flag: &str,
    detail: &str,
) -> CliResult<PathBuf> {
    match argv.next().map(PathBuf::from) {
        Some(path) => Ok(path),
        None => Err(format!("{flag} requires {detail}").into()),
    }
}

fn take_string(
    argv: &mut impl Iterator<Item = OsString>,
    flag: &str,
    detail: &str,
) -> CliResult<String> {
    match argv.next().and_then(|value| value.into_string().ok()) {
        Some(text) => Ok(text),
        None => Err(format!("{flag} requires {detail}").into()),
    }
}

fn parse_next<T: std::str::FromStr>(
    args: &mut impl Iterator<Item = OsString>,
    flag: &str,
) -> CliResult<T>
where
    T::Err: Error + Send + Sync + 'static,
{
    let text = match args.next().and_then(|value| value.into_string().ok()) {
        Some(text) => text,
        None => return Err(format!("{flag} requires a value").into()),
    };
    T::from_str(&text).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two progress lines are emitted stderr values, so their wording is
    /// interface (docs/decisions.md D-008/D-009). These tests assert the
    /// rendered bytes a consumer sees, not the source text: a re-worded label
    /// or a changed precision spec fails here. The pipeline itself cannot be
    /// driven this cheaply because the promotion gate requires 4096 held-out
    /// transitions per split before training starts.
    fn accumulated_epoch() -> TrainingEpochAccumulator {
        TrainingEpochAccumulator {
            seen_samples: 64,
            seen_batches: 2,
            candidate_total: 64.0,
            candidate_nll: 128.0,
            candidate_variance: 192.0,
            candidate_covariance: 256.0,
            candidate_grounding: 320.0,
            baseline_total: 384.0,
            baseline_nll: 448.0,
        }
    }

    #[test]
    fn epoch_progress_line_matches_the_reference_interface() {
        assert_eq!(
            epoch_log_line(2, 7, "cuda", &accumulated_epoch(), 1.25, 51.2),
            "jepa train epoch=2/7 backend=cuda samples=64 batches=2 cnn_total=1.000000 \
             cnn_nll=2.000000 cnn_var=3.000000 cnn_cov=4.000000 cnn_ground=5.000000 \
             flat_total=6.000000 flat_nll=7.000000 seconds=1.250 samples_per_second=51.20"
        );
    }

    #[test]
    fn calibration_progress_line_matches_the_reference_interface() {
        assert_eq!(
            calibration_log_line(4_096),
            "jepa calibrate constant_baseline_samples=4096"
        );
    }
}
