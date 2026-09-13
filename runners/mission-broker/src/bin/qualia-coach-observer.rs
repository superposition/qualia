//! Read-only operator guidance over measured live sensor/model snapshots.
use std::{path::PathBuf, process::ExitCode, time::Duration};
use qualia_mission_broker::{config::CoachConfig, observer, redact};

fn main() -> ExitCode {
    let mut source = None;
    let mut evidence = None;
    let mut ticks = 30;
    let mut interval = 15_000;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--status" => source = args.next().map(PathBuf::from),
            "--evidence" => evidence = args.next().map(PathBuf::from),
            "--ticks" => ticks = args.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            "--poll-ms" => interval = args.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            _ => { eprintln!("Use --status PATH --evidence NEW.jsonl [--ticks 1..60] [--poll-ms 5000..60000]"); return ExitCode::FAILURE; }
        }
    }
    let (Some(source), Some(evidence)) = (source, evidence) else { eprintln!("--status and --evidence are required"); return ExitCode::FAILURE; };
    if !(1..=60).contains(&ticks) || !(5000..=60000).contains(&interval) { eprintln!("observer bounds refused"); return ExitCode::FAILURE; }
    match observer::run(CoachConfig::from_env(), &source, &evidence, ticks, Duration::from_millis(interval)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => { eprintln!("{}", redact::secrets(&error)); ExitCode::FAILURE }
    }
}
