//! The `qualia-agent` binary: resolve the environment, then serve the surface.
//!
//! Exit codes are part of the interface a supervisor keys on: `1` when the
//! configuration cannot be honoured, `0` when the listener shuts down cleanly.
//! A TLS keypair that does not load is a start failure the reference panics on.

use qualia_agent::config::AgentConfig;

#[tokio::main]
async fn main() {
    let config = AgentConfig::from_env();
    match qualia_agent::run(config).await {
        Ok(()) => {}
        Err(error) => {
            eprintln!("qualia-agent: {error}");
            std::process::exit(1);
        }
    }
}
