//! Command grammar for the `qualia` operator binary.
//!
//! The grammar is small enough to parse by hand, and hand parsing keeps the
//! diagnostics exact: scripts match on these strings, so they are part of the
//! interface rather than an implementation detail worth hiding behind a derive.

use std::path::PathBuf;

/// One parsed invocation of the operator binary.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Start { manifest: Option<PathBuf> },
    VerifySync,
    Status { manifest: Option<PathBuf> },
    Stop { manifest: Option<PathBuf> },
    Logs { runner: String, lines: usize },
    Host,
    Health,
    Cuda,
    Planner,
    Propose(Proposal),
    Decide(Decision),
    World { json: bool },
    Decisions { json: bool, limit: usize },
    Help,
}

/// The proposal shapes `qualia propose` accepts.
#[derive(Debug, Clone, PartialEq)]
pub enum Proposal {
    NavGoal {
        x_m: f32,
        y_m: f32,
        z_m: f32,
        yaw_rad: f32,
        shared: ProposalShared,
    },
    Object {
        label: String,
        x_m: f32,
        y_m: f32,
        z_m: f32,
        yaw_rad: f32,
        marks: ObjectMarks,
        severity: Option<f32>,
        summary: Option<String>,
        shared: ProposalShared,
    },
}

/// The flags every proposal carries.
#[derive(Debug, Clone, PartialEq)]
pub struct ProposalShared {
    pub proposal_id: Option<String>,
    pub replica_id: Option<String>,
    pub belief_weight: f32,
    pub source_weight: f32,
    pub mission_relevance: f32,
    pub confidence: f32,
    pub json: bool,
}

impl Default for ProposalShared {
    fn default() -> Self {
        Self {
            proposal_id: None,
            replica_id: None,
            belief_weight: 0.8,
            source_weight: 0.9,
            mission_relevance: 0.7,
            confidence: 0.85,
            json: false,
        }
    }
}

impl Proposal {
    pub fn shared(&self) -> &ProposalShared {
        match self {
            Proposal::NavGoal { shared, .. } | Proposal::Object { shared, .. } => shared,
        }
    }
}

/// Scene roles an injected object can carry, accumulated from bare flags.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ObjectMarks {
    pub pose: bool,
    pub nav_goal: bool,
    pub hazard: bool,
}

/// The decision shapes `qualia decide` accepts.
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Promote {
        proposal_id: String,
        shared: DecisionShared,
    },
    Reject {
        proposal_id: String,
        shared: DecisionShared,
    },
}

/// The flags every decision carries.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DecisionShared {
    pub decision_id: Option<String>,
    pub output_id: Option<String>,
    pub replica_id: Option<String>,
    pub reason: Option<String>,
    pub json: bool,
}

impl Decision {
    pub fn shared(&self) -> &DecisionShared {
        match self {
            Decision::Promote { shared, .. } | Decision::Reject { shared, .. } => shared,
        }
    }
}

/// Parse an argument list, excluding the program name.
pub fn parse<I>(args: I) -> Result<Command, String>
where
    I: IntoIterator<Item = String>,
{
    let mut args = args.into_iter();
    let Some(name) = args.next() else {
        return Ok(Command::Help);
    };

    match name.as_str() {
        "run" => Ok(Command::Start {
            manifest: manifest_flag(args)?,
        }),
        "verify-sync" => bare(args, Command::VerifySync),
        "status" => Ok(Command::Status {
            manifest: manifest_flag(args)?,
        }),
        "stop" => Ok(Command::Stop {
            manifest: manifest_flag(args)?,
        }),
        "logs" => logs(args),
        "host" => bare(args, Command::Host),
        "health" => bare(args, Command::Health),
        "cuda" => bare(args, Command::Cuda),
        "planner" => bare(args, Command::Planner),
        "propose" => propose(args),
        "decide" => decide(args),
        "world" | "scene" => world(args),
        "decisions" => decisions(args),
        "-h" | "--help" | "help" => Ok(Command::Help),
        other => Err(format!("unknown command '{other}'")),
    }
}

/// A command that takes no arguments beyond an optional help flag.
fn bare<I>(args: I, command: Command) -> Result<Command, String>
where
    I: IntoIterator<Item = String>,
{
    let mut rest = args.into_iter();
    match rest.next() {
        None => Ok(command),
        Some(arg) if arg == "-h" || arg == "--help" => Ok(Command::Help),
        Some(arg) => Err(format!("unknown argument '{arg}'")),
    }
}

fn logs<I>(args: I) -> Result<Command, String>
where
    I: IntoIterator<Item = String>,
{
    let mut lines = 80usize;
    let mut runner = None;
    let mut rest = args.into_iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--lines" => {
                let raw = value(&mut rest, "--lines")?;
                lines = raw
                    .parse::<usize>()
                    .map_err(|_| format!("invalid --lines value '{raw}'"))?;
            }
            "-h" | "--help" => return Ok(Command::Help),
            _ if runner.is_none() => runner = Some(arg),
            other => return Err(format!("unknown qualia logs argument '{other}'")),
        }
    }
    let runner = runner.ok_or_else(|| "qualia logs requires a runner name".to_string())?;
    Ok(Command::Logs { runner, lines })
}

/// Parse `--manifest <path>`; a help flag here is reported the same way the
/// manifest reader reports it, which is why it is an error rather than a help.
fn manifest_flag<I>(args: I) -> Result<Option<PathBuf>, String>
where
    I: IntoIterator<Item = String>,
{
    let mut manifest = None;
    let mut rest = args.into_iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--manifest" => {
                let path = rest
                    .next()
                    .ok_or_else(|| "--manifest requires a path".to_string())?;
                manifest = Some(PathBuf::from(path));
            }
            "-h" | "--help" => return Err("help".to_string()),
            other => return Err(format!("unknown argument '{other}'")),
        }
    }
    Ok(manifest)
}

fn propose<I>(args: I) -> Result<Command, String>
where
    I: IntoIterator<Item = String>,
{
    let mut rest = args.into_iter();
    let Some(shape) = rest.next() else {
        return Err("qualia propose requires a subcommand".to_string());
    };
    match shape.as_str() {
        "nav-goal" | "goal" => propose_nav_goal(rest),
        "object" => propose_object(rest),
        "-h" | "--help" => Ok(Command::Help),
        other => Err(format!("unknown qualia propose subcommand '{other}'")),
    }
}

fn propose_nav_goal<I>(args: I) -> Result<Command, String>
where
    I: IntoIterator<Item = String>,
{
    let mut x_m = None;
    let mut y_m = 0.0;
    let mut z_m = None;
    let mut yaw_rad = 0.0;
    let mut shared = ProposalShared::default();
    let mut rest = args.into_iter();

    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--x" => x_m = Some(float(&mut rest, "--x")?),
            "--y" => y_m = float(&mut rest, "--y")?,
            "--z" => z_m = Some(float(&mut rest, "--z")?),
            "--yaw" => yaw_rad = float(&mut rest, "--yaw")?,
            "-h" | "--help" => return Ok(Command::Help),
            _ => {
                if !proposal_shared_arg(&arg, &mut rest, &mut shared)? {
                    return Err(format!("unknown qualia propose nav-goal argument '{arg}'"));
                }
            }
        }
    }

    Ok(Command::Propose(Proposal::NavGoal {
        x_m: x_m.ok_or_else(|| "qualia propose nav-goal requires --x".to_string())?,
        y_m,
        z_m: z_m.ok_or_else(|| "qualia propose nav-goal requires --z".to_string())?,
        yaw_rad,
        shared,
    }))
}

fn propose_object<I>(args: I) -> Result<Command, String>
where
    I: IntoIterator<Item = String>,
{
    let mut label = None;
    let mut x_m = None;
    let mut y_m = 0.0;
    let mut z_m = None;
    let mut yaw_rad = 0.0;
    let mut marks = ObjectMarks::default();
    let mut severity = None;
    let mut summary = None;
    let mut shared = ProposalShared::default();
    let mut rest = args.into_iter();

    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--label" => label = Some(value(&mut rest, "--label")?),
            "--x" => x_m = Some(float(&mut rest, "--x")?),
            "--y" => y_m = float(&mut rest, "--y")?,
            "--z" => z_m = Some(float(&mut rest, "--z")?),
            "--yaw" => yaw_rad = float(&mut rest, "--yaw")?,
            "--pose" => marks.pose = true,
            "--nav-goal" => marks.nav_goal = true,
            "--hazard" => marks.hazard = true,
            "--severity" => severity = Some(float(&mut rest, "--severity")?),
            "--summary" => summary = Some(value(&mut rest, "--summary")?),
            "-h" | "--help" => return Ok(Command::Help),
            _ => {
                if !proposal_shared_arg(&arg, &mut rest, &mut shared)? {
                    return Err(format!("unknown qualia propose object argument '{arg}'"));
                }
            }
        }
    }

    Ok(Command::Propose(Proposal::Object {
        label: label.ok_or_else(|| "qualia propose object requires --label".to_string())?,
        x_m: x_m.ok_or_else(|| "qualia propose object requires --x".to_string())?,
        y_m,
        z_m: z_m.ok_or_else(|| "qualia propose object requires --z".to_string())?,
        yaw_rad,
        marks,
        severity,
        summary,
        shared,
    }))
}

fn decide<I>(args: I) -> Result<Command, String>
where
    I: IntoIterator<Item = String>,
{
    let mut rest = args.into_iter();
    let Some(shape) = rest.next() else {
        return Err("qualia decide requires a subcommand".to_string());
    };
    match shape.as_str() {
        "promote" => decide_with(rest, true),
        "reject" => decide_with(rest, false),
        "-h" | "--help" => Ok(Command::Help),
        other => Err(format!("unknown qualia decide subcommand '{other}'")),
    }
}

fn decide_with<I>(args: I, promote: bool) -> Result<Command, String>
where
    I: IntoIterator<Item = String>,
{
    let verb = if promote { "promote" } else { "reject" };
    let mut rest = args.into_iter();
    let proposal_id = match rest.next() {
        Some(arg) if arg == "-h" || arg == "--help" => return Ok(Command::Help),
        Some(arg) => arg,
        None => return Err(format!("qualia decide {verb} requires a proposal id")),
    };

    let mut shared = DecisionShared::default();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(Command::Help),
            _ => {
                if !decision_shared_arg(&arg, &mut rest, &mut shared)? {
                    return Err(format!("unknown qualia decide {verb} argument '{arg}'"));
                }
            }
        }
    }

    Ok(Command::Decide(if promote {
        Decision::Promote {
            proposal_id,
            shared,
        }
    } else {
        Decision::Reject {
            proposal_id,
            shared,
        }
    }))
}

fn world<I>(args: I) -> Result<Command, String>
where
    I: IntoIterator<Item = String>,
{
    let mut json = false;
    let mut rest = args.into_iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--json" => json = true,
            "-h" | "--help" => return Ok(Command::Help),
            other => return Err(format!("unknown qualia world argument '{other}'")),
        }
    }
    Ok(Command::World { json })
}

fn decisions<I>(args: I) -> Result<Command, String>
where
    I: IntoIterator<Item = String>,
{
    let mut json = false;
    let mut limit = 128usize;
    let mut rest = args.into_iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--json" => json = true,
            "--limit" => {
                let raw = value(&mut rest, "--limit")?;
                limit = raw
                    .parse::<usize>()
                    .map_err(|_| format!("invalid --limit value '{raw}'"))?;
            }
            "-h" | "--help" => return Ok(Command::Help),
            other => return Err(format!("unknown qualia decisions argument '{other}'")),
        }
    }
    Ok(Command::Decisions { json, limit })
}

/// Absorb one of the flags shared by both proposal shapes.
fn proposal_shared_arg<I>(arg: &str, rest: &mut I, shared: &mut ProposalShared) -> Result<bool, String>
where
    I: Iterator<Item = String>,
{
    match arg {
        "--proposal-id" => shared.proposal_id = Some(value(rest, "--proposal-id")?),
        "--replica-id" => shared.replica_id = Some(value(rest, "--replica-id")?),
        "--belief-weight" => shared.belief_weight = float(rest, "--belief-weight")?,
        "--source-weight" => shared.source_weight = float(rest, "--source-weight")?,
        "--mission-relevance" => shared.mission_relevance = float(rest, "--mission-relevance")?,
        "--confidence" => shared.confidence = float(rest, "--confidence")?,
        "--json" => shared.json = true,
        _ => return Ok(false),
    }
    Ok(true)
}

/// Absorb one of the flags shared by both decision shapes.
fn decision_shared_arg<I>(arg: &str, rest: &mut I, shared: &mut DecisionShared) -> Result<bool, String>
where
    I: Iterator<Item = String>,
{
    match arg {
        "--decision-id" => shared.decision_id = Some(value(rest, "--decision-id")?),
        "--output-id" => shared.output_id = Some(value(rest, "--output-id")?),
        "--replica-id" => shared.replica_id = Some(value(rest, "--replica-id")?),
        "--reason" => shared.reason = Some(value(rest, "--reason")?),
        "--json" => shared.json = true,
        _ => return Ok(false),
    }
    Ok(true)
}

fn value<I>(rest: &mut I, flag: &str) -> Result<String, String>
where
    I: Iterator<Item = String>,
{
    rest.next()
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn float<I>(rest: &mut I, flag: &str) -> Result<f32, String>
where
    I: Iterator<Item = String>,
{
    let raw = value(rest, flag)?;
    raw.parse::<f32>()
        .map_err(|_| format!("invalid {flag} value '{raw}'"))
}

/// Print the operator help. It goes to stderr so that a piping operator still
/// sees the diagnostics that accompany it.
pub fn print_usage() {
    let lines = [
        "Usage:",
        "  qualia run [--manifest <path>]",
        "  qualia verify-sync",
        "  qualia status [--manifest <path>]",
        "  qualia stop [--manifest <path>]",
        "  qualia logs <runner> [--lines <n>]",
        "  qualia host",
        "  qualia health",
        "  qualia cuda",
        "  qualia planner",
        "  qualia propose nav-goal --x <m> --z <m> [--y <m>] [--yaw <rad>]",
        "  qualia propose object --label <name> --x <m> --z <m> [--y <m>] [--yaw <rad>]",
        "  qualia decide promote <proposal-id> [--output-id <canonical-id>]",
        "  qualia decide reject <proposal-id>",
        "  qualia world",
        "  qualia decisions [--limit <n>]",
        "",
        "Commands:",
        "  run     Start the qualia stack through qualia-init",
        "  verify-sync  Run the trusted sync verification suite",
        "  status  Show runner status from the stack manifest",
        "  stop    Gracefully stop the stack through the control socket",
        "  logs    Show recent log lines for a runner",
        "  host    Show local host and GPU toolchain details",
        "  health  Show aggregate stack health and readiness",
        "  cuda    Show CUDA service readiness and device details",
        "  planner Show planner readiness and last result",
        "  propose Inject a world-model proposal into the local scene",
        "  decide  Promote or reject an injected proposal",
        "  world   Show proposal, canonical, and operational scene state",
        "  decisions Show recorded coach decisions",
        "  help    Show this message",
    ];
    for line in lines {
        eprintln!("{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_args(args: &[&str]) -> Result<Command, String> {
        parse(args.iter().map(|arg| arg.to_string()))
    }

    #[test]
    fn empty_argv_is_help() {
        assert_eq!(parse_args(&[]), Ok(Command::Help));
    }

    #[test]
    fn every_bare_subcommand_round_trips() {
        assert_eq!(parse_args(&["verify-sync"]), Ok(Command::VerifySync));
        assert_eq!(parse_args(&["host"]), Ok(Command::Host));
        assert_eq!(parse_args(&["health"]), Ok(Command::Health));
        assert_eq!(parse_args(&["cuda"]), Ok(Command::Cuda));
        assert_eq!(parse_args(&["planner"]), Ok(Command::Planner));
    }

    #[test]
    fn manifest_flag_reaches_start_status_and_stop() {
        let path = Some(PathBuf::from("config/stack.json"));
        assert_eq!(
            parse_args(&["run", "--manifest", "config/stack.json"]),
            Ok(Command::Start {
                manifest: path.clone()
            })
        );
        assert_eq!(
            parse_args(&["status", "--manifest", "config/stack.json"]),
            Ok(Command::Status {
                manifest: path.clone()
            })
        );
        assert_eq!(
            parse_args(&["stop", "--manifest", "config/stack.json"]),
            Ok(Command::Stop { manifest: path })
        );
    }

    #[test]
    fn manifest_defaults_to_none() {
        assert_eq!(parse_args(&["run"]), Ok(Command::Start { manifest: None }));
        assert_eq!(
            parse_args(&["status"]),
            Ok(Command::Status { manifest: None })
        );
        assert_eq!(parse_args(&["stop"]), Ok(Command::Stop { manifest: None }));
    }

    #[test]
    fn logs_defaults_to_eighty_lines_and_accepts_a_count() {
        assert_eq!(
            parse_args(&["logs", "qualia-agent"]),
            Ok(Command::Logs {
                runner: "qualia-agent".to_string(),
                lines: 80,
            })
        );
        assert_eq!(
            parse_args(&["logs", "qualia-agent", "--lines", "20"]),
            Ok(Command::Logs {
                runner: "qualia-agent".to_string(),
                lines: 20,
            })
        );
    }

    #[test]
    fn propose_nav_goal_defaults_y_and_yaw_to_zero() {
        let command = parse_args(&[
            "propose", "nav-goal", "--x", "4.0", "--z", "-1.5", "--yaw", "0.25",
        ])
        .expect("parse nav goal");
        assert_eq!(
            command,
            Command::Propose(Proposal::NavGoal {
                x_m: 4.0,
                y_m: 0.0,
                z_m: -1.5,
                yaw_rad: 0.25,
                shared: ProposalShared::default(),
            })
        );
    }

    #[test]
    fn goal_is_an_alias_for_nav_goal() {
        let command = parse_args(&["propose", "goal", "--x", "1.0", "--z", "2.0"]);
        assert_eq!(
            command,
            Ok(Command::Propose(Proposal::NavGoal {
                x_m: 1.0,
                y_m: 0.0,
                z_m: 2.0,
                yaw_rad: 0.0,
                shared: ProposalShared::default(),
            }))
        );
    }

    #[test]
    fn propose_object_collects_marks_severity_and_summary() {
        let command = parse_args(&[
            "propose",
            "object",
            "--label",
            "crate",
            "--x",
            "1.0",
            "--z",
            "2.5",
            "--hazard",
            "--severity",
            "0.6",
            "--summary",
            "wet floor",
        ])
        .expect("parse object");
        assert_eq!(
            command,
            Command::Propose(Proposal::Object {
                label: "crate".to_string(),
                x_m: 1.0,
                y_m: 0.0,
                z_m: 2.5,
                yaw_rad: 0.0,
                marks: ObjectMarks {
                    pose: false,
                    nav_goal: false,
                    hazard: true,
                },
                severity: Some(0.6),
                summary: Some("wet floor".to_string()),
                shared: ProposalShared::default(),
            })
        );
    }

    #[test]
    fn proposal_weight_overrides_are_shared_by_both_shapes() {
        let expected = ProposalShared {
            proposal_id: Some("proposal/fixed".to_string()),
            replica_id: Some("operator-a".to_string()),
            belief_weight: 0.5,
            source_weight: 0.6,
            mission_relevance: 0.7,
            confidence: 0.4,
            json: true,
        };
        let extra = [
            "--proposal-id",
            "proposal/fixed",
            "--replica-id",
            "operator-a",
            "--belief-weight",
            "0.5",
            "--source-weight",
            "0.6",
            "--mission-relevance",
            "0.7",
            "--confidence",
            "0.4",
            "--json",
        ];

        let mut nav = vec!["propose", "nav-goal", "--x", "1.0", "--z", "2.0"];
        nav.extend(extra);
        assert_eq!(
            parse_args(&nav),
            Ok(Command::Propose(Proposal::NavGoal {
                x_m: 1.0,
                y_m: 0.0,
                z_m: 2.0,
                yaw_rad: 0.0,
                shared: expected.clone(),
            }))
        );

        let mut object = vec![
            "propose", "object", "--label", "crate", "--x", "1.0", "--z", "2.0",
        ];
        object.extend(extra);
        assert_eq!(
            parse_args(&object),
            Ok(Command::Propose(Proposal::Object {
                label: "crate".to_string(),
                x_m: 1.0,
                y_m: 0.0,
                z_m: 2.0,
                yaw_rad: 0.0,
                marks: ObjectMarks::default(),
                severity: None,
                summary: None,
                shared: expected,
            }))
        );
    }

    #[test]
    fn decide_promote_and_reject_bind_a_proposal_id() {
        assert_eq!(
            parse_args(&[
                "decide",
                "promote",
                "proposal/nav-goal-1",
                "--output-id",
                "world/nav-goal",
            ]),
            Ok(Command::Decide(Decision::Promote {
                proposal_id: "proposal/nav-goal-1".to_string(),
                shared: DecisionShared {
                    output_id: Some("world/nav-goal".to_string()),
                    ..DecisionShared::default()
                },
            }))
        );
        assert_eq!(
            parse_args(&[
                "decide",
                "reject",
                "proposal/crate-1",
                "--reason",
                "not needed",
            ]),
            Ok(Command::Decide(Decision::Reject {
                proposal_id: "proposal/crate-1".to_string(),
                shared: DecisionShared {
                    reason: Some("not needed".to_string()),
                    ..DecisionShared::default()
                },
            }))
        );
    }

    #[test]
    fn world_and_scene_share_the_json_switch() {
        assert_eq!(parse_args(&["world"]), Ok(Command::World { json: false }));
        assert_eq!(parse_args(&["scene"]), Ok(Command::World { json: false }));
        assert_eq!(
            parse_args(&["world", "--json"]),
            Ok(Command::World { json: true })
        );
    }

    #[test]
    fn decisions_defaults_to_a_hundred_and_twenty_eight() {
        assert_eq!(
            parse_args(&["decisions"]),
            Ok(Command::Decisions {
                json: false,
                limit: 128,
            })
        );
        assert_eq!(
            parse_args(&["decisions", "--json", "--limit", "5"]),
            Ok(Command::Decisions {
                json: true,
                limit: 5,
            })
        );
    }

    #[test]
    fn help_spellings_are_accepted_at_the_top_level() {
        for spelling in ["help", "-h", "--help"] {
            assert_eq!(parse_args(&[spelling]), Ok(Command::Help), "{spelling}");
        }
    }

    #[test]
    fn subcommand_help_is_help_except_where_a_manifest_is_expected() {
        assert_eq!(parse_args(&["propose", "--help"]), Ok(Command::Help));
        assert_eq!(
            parse_args(&["propose", "nav-goal", "--help"]),
            Ok(Command::Help)
        );
        assert_eq!(parse_args(&["decide", "--help"]), Ok(Command::Help));
        assert_eq!(parse_args(&["logs", "--help"]), Ok(Command::Help));
        assert_eq!(parse_args(&["world", "--help"]), Ok(Command::Help));
        // The manifest readers report help through the error channel.
        assert_eq!(parse_args(&["run", "--help"]).unwrap_err(), "help");
        assert_eq!(parse_args(&["status", "--help"]).unwrap_err(), "help");
        assert_eq!(parse_args(&["stop", "--help"]).unwrap_err(), "help");
    }

    #[test]
    fn unknown_commands_and_arguments_are_named_in_the_error() {
        assert_eq!(
            parse_args(&["bogus"]).unwrap_err(),
            "unknown command 'bogus'"
        );
        assert_eq!(parse_args(&["host", "extra"]).unwrap_err(), "unknown argument 'extra'");
        assert_eq!(
            parse_args(&["logs", "r", "--bogus"]).unwrap_err(),
            "unknown qualia logs argument '--bogus'"
        );
        assert_eq!(
            parse_args(&["propose"]).unwrap_err(),
            "qualia propose requires a subcommand"
        );
        assert_eq!(
            parse_args(&["propose", "bogus"]).unwrap_err(),
            "unknown qualia propose subcommand 'bogus'"
        );
        assert_eq!(
            parse_args(&["decide"]).unwrap_err(),
            "qualia decide requires a subcommand"
        );
        assert_eq!(
            parse_args(&["decide", "bogus"]).unwrap_err(),
            "unknown qualia decide subcommand 'bogus'"
        );
        assert_eq!(
            parse_args(&["world", "bogus"]).unwrap_err(),
            "unknown qualia world argument 'bogus'"
        );
        assert_eq!(
            parse_args(&["decisions", "bogus"]).unwrap_err(),
            "unknown qualia decisions argument 'bogus'"
        );
    }

    #[test]
    fn required_flags_and_values_are_enforced() {
        assert_eq!(
            parse_args(&["propose", "nav-goal", "--z", "1.0"]).unwrap_err(),
            "qualia propose nav-goal requires --x"
        );
        assert_eq!(
            parse_args(&["propose", "object", "--label", "c", "--z", "1.0"]).unwrap_err(),
            "qualia propose object requires --x"
        );
        assert_eq!(
            parse_args(&["decide", "promote"]).unwrap_err(),
            "qualia decide promote requires a proposal id"
        );
        assert_eq!(
            parse_args(&["logs", "r", "--lines"]).unwrap_err(),
            "--lines requires a value"
        );
        assert_eq!(
            parse_args(&["propose", "nav-goal", "--x"]).unwrap_err(),
            "--x requires a value"
        );
        assert_eq!(
            parse_args(&["propose", "nav-goal", "--x", "north"]).unwrap_err(),
            "invalid --x value 'north'"
        );
        assert_eq!(
            parse_args(&["decisions", "--limit", "lots"]).unwrap_err(),
            "invalid --limit value 'lots'"
        );
        assert_eq!(
            parse_args(&["status", "--manifest"]).unwrap_err(),
            "--manifest requires a path"
        );
        assert_eq!(
            parse_args(&["status", "extra"]).unwrap_err(),
            "unknown argument 'extra'"
        );
    }
}
