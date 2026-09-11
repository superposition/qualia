//! `qualia-jepa-registry` command line: register, promote, health, rollback,
//! reconcile and status over one Turso registry file.

use qualia_jepa_registry::{now_ms, CandidateRegistry, DEFAULT_MAX_REPORT_AGE_MS};
use std::env;
use std::ffi::OsString;
use std::path::PathBuf;

type CliResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Everything the operator may set on the command line.
struct Options {
    db: PathBuf,
    generation_file: PathBuf,
    checkpoint_dir: Option<PathBuf>,
    checkpoint_id: Option<String>,
    dataset_manifest: Option<PathBuf>,
    generation: Option<u64>,
    max_age_hours: u128,
    healthy: Option<bool>,
    reason: String,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            db: PathBuf::from("artifacts/jepa/registry.turso"),
            generation_file: PathBuf::from("artifacts/jepa/active-generation.json"),
            checkpoint_dir: None,
            checkpoint_id: None,
            dataset_manifest: None,
            generation: None,
            max_age_hours: 168,
            healthy: None,
            reason: "operator registry action".to_string(),
        }
    }
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("qualia-jepa-registry: {error:#}");
        std::process::exit(1);
    }
}

async fn run() -> CliResult<()> {
    let mut args = env::args_os().skip(1);
    let command = args
        .next()
        .and_then(|value| value.into_string().ok())
        .ok_or("a registry command is required")?;
    if matches!(command.as_str(), "--help" | "-h" | "help") {
        print_help();
        return Ok(());
    }
    let options = parse_options(&mut args)?;

    let registry = CandidateRegistry::open(&options.db).await?;
    let now = now_ms();
    let output = match command.as_str() {
        "register" => serde_json::to_value(
            registry
                .register(
                    options
                        .checkpoint_dir
                        .ok_or("--checkpoint-dir is required")?,
                    options
                        .dataset_manifest
                        .ok_or("--dataset-manifest is required")?,
                    now,
                    options
                        .max_age_hours
                        .checked_mul(60 * 60 * 1_000)
                        .unwrap_or(DEFAULT_MAX_REPORT_AGE_MS),
                )
                .await?,
        )?,
        "promote" => serde_json::to_value(
            registry
                .promote(
                    &options.checkpoint_id.ok_or("--checkpoint-id is required")?,
                    &options.generation_file,
                    now,
                )
                .await?,
        )?,
        "health" => serde_json::to_value(
            registry
                .record_health(
                    &options.generation_file,
                    options.generation.ok_or("--generation is required")?,
                    options
                        .healthy
                        .ok_or("--healthy or --failed is required")?,
                    now,
                    &options.reason,
                )
                .await?,
        )?,
        "rollback" => serde_json::to_value(
            registry
                .rollback(&options.generation_file, now, &options.reason)
                .await?,
        )?,
        "reconcile" => {
            serde_json::to_value(registry.reconcile(&options.generation_file, now).await?)?
        }
        "status" => serde_json::to_value(
            registry
                .status(
                    options
                        .generation_file
                        .exists()
                        .then_some(options.generation_file.as_path()),
                )
                .await?,
        )?,
        _ => return Err("unknown registry command".into()),
    };
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

fn parse_options(args: &mut impl Iterator<Item = OsString>) -> CliResult<Options> {
    let mut options = Options::default();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--db") => options.db = next_path(args, "--db")?,
            Some("--generation-file") => {
                options.generation_file = next_path(args, "--generation-file")?
            }
            Some("--checkpoint-dir") => {
                options.checkpoint_dir = Some(next_path(args, "--checkpoint-dir")?)
            }
            Some("--checkpoint-id") => {
                options.checkpoint_id = Some(next_string(args, "--checkpoint-id")?)
            }
            Some("--dataset-manifest") => {
                options.dataset_manifest = Some(next_path(args, "--dataset-manifest")?)
            }
            Some("--generation") => options.generation = Some(next_parse(args, "--generation")?),
            Some("--max-age-hours") => {
                options.max_age_hours = next_parse(args, "--max-age-hours")?
            }
            Some("--healthy") => options.healthy = Some(true),
            Some("--failed") => options.healthy = Some(false),
            Some("--reason") => options.reason = next_string(args, "--reason")?,
            _ => return Err("unknown or incomplete registry argument".into()),
        }
    }
    Ok(options)
}

fn next_path(args: &mut impl Iterator<Item = OsString>, flag: &str) -> CliResult<PathBuf> {
    Ok(PathBuf::from(
        args.next()
            .ok_or_else(|| format!("{flag} requires a path"))?,
    ))
}

fn next_string(args: &mut impl Iterator<Item = OsString>, flag: &str) -> CliResult<String> {
    args.next()
        .and_then(|value| value.into_string().ok())
        .ok_or_else(|| format!("{flag} requires UTF-8 text").into())
}

fn next_parse<T: std::str::FromStr>(
    args: &mut impl Iterator<Item = OsString>,
    flag: &str,
) -> CliResult<T>
where
    T::Err: std::error::Error + Send + Sync + 'static,
{
    Ok(next_string(args, flag)?.parse()?)
}

fn print_help() {
    println!(
        "qualia-jepa-registry <register|promote|health|rollback|reconcile|status> [options]\n\
         register --checkpoint-dir DIR --dataset-manifest FILE [--max-age-hours N]\n\
         promote --checkpoint-id ID [--generation-file FILE]\n\
         health --generation N (--healthy|--failed) [--reason TEXT]\n\
         rollback [--reason TEXT]\n\
         reconcile\n\
         status\n\
         common: [--db FILE] [--generation-file FILE]"
    );
}
