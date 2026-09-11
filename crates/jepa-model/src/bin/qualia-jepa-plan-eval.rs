//! Offline rollout proposal and planner comparison.
//!
//! `propose` scores a candidate's rollout request against a verified immutable
//! checkpoint and writes a proposal-only artifact. `compare` scores the
//! candidate against the incumbent planner over recorded decisions whose MCAP
//! evidence is re-hashed. Both write a brand-new file and refuse to overwrite.

use qualia_jepa_dataset::{manifest_digest, validate_dataset_manifest_integrity, DatasetManifest};
use qualia_jepa_model::planner::{
    compare_offline_planners, evaluate_rollout_proposals, OfflinePlannerComparisonConfig,
    OfflinePlannerComparisonProvenance, OfflinePlannerDecision, RolloutEvaluationRequest,
};
use qualia_jepa_model::runtime::CoherentJepaRuntime;
use qualia_jepa_model::{
    device_for_backend, CheckpointManifest, TrainingReport, DEFAULT_MAX_TRAINING_REPORT_AGE_MS,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

type CliResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

const PROPOSAL_REQUEST_SCHEMA: &str = "qualia.jepa-plan-evaluation-request.v1";
const COMPARISON_INPUT_SCHEMA: &str = "qualia.jepa-planner-comparison-input.v1";

#[derive(Debug)]
enum Command {
    Propose {
        checkpoint: PathBuf,
        backend: String,
        request: PathBuf,
        output: PathBuf,
        operator_enabled: bool,
    },
    Compare {
        input: PathBuf,
        dataset: PathBuf,
        checkpoint: PathBuf,
        existing_planner_config: PathBuf,
        output: PathBuf,
    },
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProposalRequestFile {
    schema_version: String,
    rollout: RolloutEvaluationRequest,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ComparisonInputFile {
    schema_version: String,
    provenance: OfflinePlannerComparisonProvenance,
    config: OfflinePlannerComparisonConfig,
    decisions: Vec<OfflinePlannerDecision>,
}

/// Build a `CliResult` failure carrying a fixed diagnostic message.
fn fail<T>(message: &'static str) -> CliResult<T> {
    Err(message.into())
}

/// Take the token following an option, or fail naming that option.
fn flag_value(args: &mut impl Iterator<Item = String>, flag: &str) -> CliResult<String> {
    match args.next() {
        Some(value) => Ok(value),
        None => Err(format!("{flag} requires a value").into()),
    }
}

/// Unwrap a positional option that must have been supplied.
fn require_flag(value: Option<PathBuf>, message: &'static str) -> CliResult<PathBuf> {
    match value {
        Some(path) => Ok(path),
        None => Err(message.into()),
    }
}

/// Render raw digest bytes as lowercase hexadecimal.
fn hex_digest(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut hex = String::with_capacity(2 * bytes.len());
    for byte in bytes {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

fn main() -> CliResult<()> {
    match parse_args()? {
        Command::Propose { checkpoint, backend, request, output, operator_enabled } => {
            run_proposal(&checkpoint, &backend, &request, &output, operator_enabled)
        }
        Command::Compare { input, dataset, checkpoint, existing_planner_config, output } => {
            run_comparison(&input, &dataset, &checkpoint, &existing_planner_config, &output)
        }
    }
}

fn run_proposal(
    checkpoint_dir: &Path,
    backend: &str,
    request_path: &Path,
    output: &Path,
    operator_enabled: bool,
) -> CliResult<()> {
    if !operator_enabled {
        return fail(
            "rollout proposals are disabled; pass the exact --enable-proposals operator flag",
        );
    }
    let request: ProposalRequestFile = serde_json::from_slice(&fs::read(request_path)?)?;
    if request.schema_version != PROPOSAL_REQUEST_SCHEMA {
        return fail("unsupported proposal request schema");
    }
    let manifest: CheckpointManifest =
        serde_json::from_slice(&fs::read(checkpoint_dir.join("manifest.json"))?)?;
    verify_provenance(checkpoint_dir, &manifest, &request.rollout)?;

    let runtime =
        CoherentJepaRuntime::from_checkpoint(checkpoint_dir, device_for_backend(backend)?)?;
    let requested = &request.rollout.source;
    let identity_matches = runtime.checkpoint_id() == requested.checkpoint_id
        && runtime.weights_sha256() == requested.checkpoint_sha256;
    if !identity_matches {
        return fail("loaded runtime identity does not match proposal provenance");
    }
    let proposal = evaluate_rollout_proposals(&runtime, &request.rollout, true)?;
    write_new_json(output, &proposal)
}

fn run_comparison(
    input: &Path,
    dataset_path: &Path,
    checkpoint_dir: &Path,
    existing_planner_config: &Path,
    output: &Path,
) -> CliResult<()> {
    let input_bytes = fs::read(input)?;
    let input_sha256 = hex_digest(&Sha256::digest(&input_bytes));
    let input: ComparisonInputFile = serde_json::from_slice(&input_bytes)?;
    if input.schema_version != COMPARISON_INPUT_SCHEMA {
        return fail("unsupported planner comparison input schema");
    }
    verify_comparison_evidence(&input, dataset_path, checkpoint_dir, existing_planner_config)?;
    let report = compare_offline_planners(
        &input.decisions,
        &input.config,
        &input.provenance,
        &input_sha256,
    )?;
    write_new_json(output, &report)
}

fn verify_comparison_evidence(
    input: &ComparisonInputFile,
    dataset_path: &Path,
    checkpoint_dir: &Path,
    existing_planner_config: &Path,
) -> CliResult<()> {
    let dataset: DatasetManifest = serde_json::from_slice(&fs::read(dataset_path)?)?;
    let is_immutable_v3 = dataset.schema_version == "qualia.jepa-dataset.v3"
        && dataset.digest == manifest_digest(&dataset)?;
    if !is_immutable_v3 {
        return fail("planner comparison dataset manifest is not immutable v3 evidence");
    }
    validate_dataset_manifest_integrity(&dataset)?;

    let manifest: CheckpointManifest =
        serde_json::from_slice(&fs::read(checkpoint_dir.join("manifest.json"))?)?;
    let weights_sha256 = sha256_file(&checkpoint_dir.join("weights.safetensors"))?;
    let report_bytes = fs::read(&manifest.training_report_path)?;
    let report_sha256 = hex_digest(&Sha256::digest(&report_bytes));
    let report: TrainingReport = serde_json::from_slice(&report_bytes)?;
    let now_ms = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    report.validate_for_promotion(&manifest, now_ms, DEFAULT_MAX_TRAINING_REPORT_AGE_MS)?;
    let planner_config_sha256 = sha256_file(existing_planner_config)?;

    let provenance = &input.provenance;
    let mismatches = [
        provenance.dataset_digest != dataset.digest,
        provenance.dataset_digest != manifest.dataset_digest,
        provenance.dataset_digest != manifest.baseline_gate.dataset_digest,
        provenance.checkpoint_id != manifest.checkpoint_id,
        provenance.checkpoint_sha256 != manifest.weights_sha256,
        provenance.checkpoint_sha256 != weights_sha256,
        provenance.training_report_sha256 != manifest.training_report_sha256,
        provenance.training_report_sha256 != report_sha256,
        provenance.existing_planner_config_sha256 != planner_config_sha256,
        !provenance.llm_priors_ablated,
        manifest.status != "candidate",
        !manifest.baseline_gate.passes(),
    ];
    if mismatches.iter().any(|&mismatch| mismatch) {
        return fail(
            "planner comparison provenance does not match its dataset, checkpoint, report, or planner config",
        );
    }

    let mut recorded: HashSet<(&str, &str)> = HashSet::new();
    for source in &dataset.sources {
        recorded.insert((source.session_id.as_str(), source.sha256.as_str()));
    }
    let mut cited: HashSet<(&str, &str)> = HashSet::new();
    for decision in &input.decisions {
        cited.insert((
            decision.source_session_id.as_str(),
            decision.source_mcap_sha256.as_str(),
        ));
    }
    if cited.is_empty() || !cited.is_subset(&recorded) {
        return fail("planner comparison decision references evidence outside the dataset");
    }
    for (session_id, mcap_sha256) in cited {
        let source = match dataset
            .sources
            .iter()
            .find(|row| row.session_id == session_id && row.sha256 == mcap_sha256)
        {
            Some(row) => row,
            None => return fail("planner comparison source disappeared during validation"),
        };
        if sha256_file(Path::new(&source.path))? != source.sha256 {
            return fail("planner comparison MCAP digest does not match the dataset");
        }
    }
    Ok(())
}

fn sha256_file(path: &Path) -> CliResult<String> {
    let mut reader = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut chunk = [0_u8; 1 << 16];
    loop {
        match reader.read(&mut chunk)? {
            0 => break,
            count => hasher.update(&chunk[..count]),
        }
    }
    Ok(hex_digest(&hasher.finalize()))
}

fn verify_provenance(
    checkpoint_dir: &Path,
    manifest: &CheckpointManifest,
    request: &RolloutEvaluationRequest,
) -> CliResult<()> {
    let weights_sha256 = sha256_file(&checkpoint_dir.join("weights.safetensors"))?;
    let report_bytes = fs::read(&manifest.training_report_path)?;
    let report_sha256 = hex_digest(&Sha256::digest(&report_bytes));
    let report: TrainingReport = serde_json::from_slice(&report_bytes)?;
    let requested_age_reference_ms = u128::from(request.evaluated_at_ns / 1_000_000);
    report.validate_for_promotion(
        manifest,
        requested_age_reference_ms,
        DEFAULT_MAX_TRAINING_REPORT_AGE_MS,
    )?;

    let source = &request.source;
    let mismatches = [
        source.architecture_id != manifest.architecture_id,
        source.checkpoint_id != manifest.checkpoint_id,
        source.checkpoint_sha256 != manifest.weights_sha256,
        source.checkpoint_sha256 != weights_sha256,
        source.training_report_sha256 != manifest.training_report_sha256,
        source.training_report_sha256 != report_sha256,
        source.dataset_digest != manifest.dataset_digest,
        source.dataset_digest != manifest.baseline_gate.dataset_digest,
        (request.config.grounding_resolution_m - manifest.grounding_geometry.resolution_m).abs()
            > 1.0e-6,
        manifest.status != "candidate",
        !manifest.baseline_gate.passes(),
    ];
    if mismatches.iter().any(|&mismatch| mismatch) {
        return fail("proposal provenance does not match a verified immutable checkpoint");
    }
    for candidate in &request.candidates {
        for step in &candidate.steps {
            let in_support = manifest.action_support.contains(
                step.left,
                step.right,
                step.speed_scale,
                step.dt_seconds,
            );
            if !in_support {
                return fail("proposal action is outside immutable training support");
            }
        }
    }
    Ok(())
}

/// Stage `value` as a sibling `.partial` file, flush it, and publish it with an
/// atomic rename; a pre-existing destination is never clobbered.
fn write_new_json(output: &Path, value: &impl Serialize) -> CliResult<()> {
    let parent = output.parent().unwrap_or(Path::new("."));
    if !parent.is_dir() {
        return fail("output parent directory does not exist");
    }
    if output.exists() {
        return fail("refusing to overwrite an existing evaluation artifact");
    }
    let file_name = match output.file_name().and_then(|name| name.to_str()) {
        Some(name) => name,
        None => return fail("output must have a valid file name"),
    };
    let staging = parent.join(format!(".{file_name}.partial"));
    let bytes = serde_json::to_vec_pretty(value)?;
    let mut staged = OpenOptions::new().write(true).create_new(true).open(&staging)?;
    staged.write_all(&bytes)?;
    staged.write_all(b"\n")?;
    staged.sync_all()?;
    fs::rename(&staging, output)?;
    #[cfg(unix)]
    std::fs::File::open(parent)?.sync_all()?;
    Ok(())
}

fn parse_args() -> CliResult<Command> {
    let mut args = env::args().skip(1);
    let mode = args.next().ok_or_else(|| usage().to_string())?;
    if mode == "--help" || mode == "-h" {
        println!("{}", usage());
        std::process::exit(0);
    }
    match mode.as_str() {
        "propose" => parse_propose(args),
        "compare" => parse_compare(args),
        _ => Err(usage().into()),
    }
}

fn parse_propose(mut args: impl Iterator<Item = String>) -> CliResult<Command> {
    let mut checkpoint = None;
    let mut request = None;
    let mut output = None;
    let mut backend = None;
    let mut operator_enabled = false;
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--checkpoint" => checkpoint = args.next().map(PathBuf::from),
            "--request" => request = args.next().map(PathBuf::from),
            "--output" => output = args.next().map(PathBuf::from),
            "--backend" => backend = Some(flag_value(&mut args, "--backend")?),
            "--enable-proposals" => operator_enabled = true,
            _ => return Err(format!("unknown propose argument {flag}").into()),
        }
    }
    Ok(Command::Propose {
        checkpoint: require_flag(checkpoint, "--checkpoint is required")?,
        backend: backend.unwrap_or_else(|| "cpu".to_string()),
        request: require_flag(request, "--request is required")?,
        output: require_flag(output, "--output is required")?,
        operator_enabled,
    })
}

fn parse_compare(mut args: impl Iterator<Item = String>) -> CliResult<Command> {
    let mut input = None;
    let mut dataset = None;
    let mut checkpoint = None;
    let mut existing_planner_config = None;
    let mut output = None;
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" => input = args.next().map(PathBuf::from),
            "--dataset" => dataset = args.next().map(PathBuf::from),
            "--checkpoint" => checkpoint = args.next().map(PathBuf::from),
            "--existing-planner-config" => existing_planner_config = args.next().map(PathBuf::from),
            "--output" => output = args.next().map(PathBuf::from),
            _ => return Err(format!("unknown compare argument {flag}").into()),
        }
    }
    Ok(Command::Compare {
        input: require_flag(input, "--input is required")?,
        dataset: require_flag(dataset, "--dataset is required")?,
        checkpoint: require_flag(checkpoint, "--checkpoint is required")?,
        existing_planner_config: require_flag(
            existing_planner_config,
            "--existing-planner-config is required",
        )?,
        output: require_flag(output, "--output is required")?,
    })
}

fn usage() -> &'static str {
    "usage:\n  qualia-jepa-plan-eval propose --checkpoint <dir> --request <json> \\\n     --output <new-json> [--backend cpu|metal|cuda] --enable-proposals\n  \
     qualia-jepa-plan-eval compare --input <json> --dataset <manifest> \\\n     --checkpoint <dir> --existing-planner-config <file> --output <new-json>"
}
