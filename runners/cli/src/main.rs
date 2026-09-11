//! `qualia` — the operator surface for a running qualia stack.
//!
//! The binary does four jobs: it starts the stack through the supervisor,
//! reports what the manifest says is running, talks to the control socket, and
//! reads or writes the agent's world model on an operator's behalf. Output is
//! line-oriented and stable so scripts can match on it; a service that is not
//! there is reported as unavailable rather than turning into a failure.

mod cli;
mod commands;
mod http;
mod platform;
mod world;

use std::process::ExitCode;

fn main() -> ExitCode {
    commands::dispatch(std::env::args().skip(1))
}
