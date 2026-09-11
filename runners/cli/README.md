# `runners/cli`

## Purpose
The operator command line for a qualia stack: start it, see which runners are
alive, stop it over the control socket, read the host and service health, and
inject or read world-model state through a running agent.

The binary is named `qualia` and is built from the `qualia-cli` package.

## Navigation
- [Repository Root](../../README.md)
- [Parent: `runners`](../README.md)
- [Child: `src`](src/README.md)
- [Child: `tests`](tests/README.md)

## Commands
- `qualia run [--manifest <path>]` — hand control to `qualia-init`
- `qualia verify-sync` — run the trusted sync verification script (aliased by
  `.cargo/config.toml` as `cargo verify-sync`)
- `qualia status [--manifest <path>]` — manifest details and per-runner state
- `qualia stop [--manifest <path>]` — send `Shutdown` over the control socket
- `qualia logs <runner> [--lines <n>]` — tail a runner log (`QUALIA_LOG_DIR`)
- `qualia host` — host name, CPU, memory, GPU and nvcc, or `unavailable`
- `qualia health` — aggregate readiness from `GET /health/ready`
- `qualia cuda` — compute service capabilities over `QUALIA_COMPUTE_SOCKET`
- `qualia planner` — planner readiness and the last planning result
- `qualia propose nav-goal|object ...` — inject a world-model proposal
- `qualia decide promote|reject <proposal-id>` — record a coach decision
- `qualia world [--json]` — proposal, canonical and operational scene state
- `qualia decisions [--json] [--limit <n>]` — recorded coach decisions

## Agent URL
`qualia health`, `qualia planner`, `qualia world`, `qualia decisions`,
`qualia propose` and `qualia decide` read the agent at `QUALIA_AGENT_URL` when
it is set, otherwise at `https://127.0.0.1:$QUALIA_WEB_PORT` (default `8080`).
Every request carries a connect timeout and a total timeout, so a server that
is silent, hung or trickling a body fails with a readable reason instead of
holding the terminal. When the configured scheme does not answer, the same host
is tried on the other scheme and the fallback is printed; no other host or port
is ever probed.

## Exit codes
- `0` — the command completed (including the `unavailable` service reports)
- `1` — the command could not complete: a missing manifest, log, socket,
  script or unreachable agent
- `2` — a usage error: an unknown command, flag or value
