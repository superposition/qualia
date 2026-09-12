//! `qualia-mission-broker` — one bounded broker run.
//!
//! ```text
//! qualia-mission-broker --once --proposal docs/evidence/T63/proposal-frontier-1.json
//! ```
//!
//! Configuration is the environment (see the crate README); the flags only
//! bound the run and name the braid's proposal record. A run with no key, or
//! one whose coach does not answer in time, prints one named line, produces no
//! decision and exits 0 — degradation is not a crash.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use qualia_mission_broker::broker::{self, RunOptions};
use qualia_mission_broker::config::BrokerConfig;
use qualia_mission_broker::redact;

const USAGE: &str = "\
qualia-mission-broker — the coach over the braid's proposals, and the missions it justifies

USAGE:
    qualia-mission-broker [--once | --ticks N] [--poll-ms N] [--proposal PATH]... [--no-status] [--status-port N]

FLAGS:
    --once              run one tick and exit (the acceptance path)
    --ticks N           run at most N ticks
    --poll-ms N         gap between ticks in a multi-tick run
    --proposal PATH     a JSON file of proposal envelopes to decide over
                        ({\"proposals\": [...]}, an array, or one object); repeatable
    --no-status         do not serve GET /coach for the console
    --status-port N     the status surface's port (default QUALIA_COACH_STATUS_PORT, 8091)
    -h, --help          this text

ENVIRONMENT:
    QUALIA_AGENT_URL            the agent's operator surface (default http://127.0.0.1:8080)
    QUALIA_AGENT_TLS_DIR        the agent's certificate directory ($QUALIA_TLS_DIR also read)
    QUALIA_MISSION_BROKER_TOKEN the mission-broker bearer (or ..._TOKEN_FILE)
    DEEPSEEK_API_KEY            the coach's credential (or QUALIA_COACH_API_KEY, or a _FILE path)
    QUALIA_COACH_BASE_URL       default https://api.deepseek.com
    QUALIA_COACH_MODEL          default deepseek-chat
    QUALIA_COACH_TIMEOUT_MS     default 8000
    QUALIA_COACH_STATUS_PORT    default 8091, loopback only

EXIT:
    0  the run completed, decided or degraded; the last line names which
    1  the run could not start (configuration, transport setup)
    2  the command line was not understood
";

fn main() -> ExitCode {
    let mut options = RunOptions::default();
    let mut status = true;
    let mut status_port: Option<u16> = None;
    let mut once = false;

    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--once" => once = true,
            "--ticks" => match args.next().and_then(|value| value.parse::<u32>().ok()) {
                Some(ticks) => options.ticks = ticks.max(1),
                None => return usage("--ticks needs a positive integer"),
            },
            "--poll-ms" => match args.next().and_then(|value| value.parse::<u64>().ok()) {
                Some(millis) => options.interval = Duration::from_millis(millis),
                None => return usage("--poll-ms needs a non-negative integer"),
            },
            "--proposal" => match args.next() {
                Some(path) => options.proposals.push(PathBuf::from(path)),
                None => return usage("--proposal needs a path"),
            },
            "--no-status" => status = false,
            "--status-port" => match args.next().and_then(|value| value.parse::<u16>().ok()) {
                Some(port) => status_port = Some(port),
                None => return usage("--status-port needs a port number"),
            },
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            other => return usage(&format!("unknown argument {other}")),
        }
    }
    if once && options.ticks > 1 {
        return usage("--once and --ticks are mutually exclusive");
    }
    if once {
        options.ticks = 1;
    }

    let mut config = BrokerConfig::from_env();
    if !options.proposals.is_empty() {
        config.proposals = options.proposals.clone();
    }
    options.proposals = config.proposals.clone();
    options.status = status && config.status_enabled;
    if let Some(port) = status_port {
        options.status_port = port;
    } else {
        options.status_port = config.status_port;
        options.status_host = config.status_host.clone();
    }

    match broker::run(config, options) {
        Ok(report) => {
            println!(
                "qualia-mission-broker: exit 0 state={} ticks={} decided={} posted={} degraded={}",
                report.state(),
                report.ticks,
                report.decided,
                report.posted,
                report.degradations.len()
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!(
                "qualia-mission-broker: {}",
                redact::secrets(&format!("run failed: {error}"))
            );
            ExitCode::FAILURE
        }
    }
}

fn usage(problem: &str) -> ExitCode {
    eprintln!("qualia-mission-broker: {problem}");
    eprintln!("{USAGE}");
    ExitCode::from(2)
}
