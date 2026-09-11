# `runners/cli/tests`

## Purpose
Test suite for the `qualia` operator binary.

- `cli.rs` — spawns the built binary and asserts its observable contract:
  argument parsing, exit codes, report shape, the control-socket frame, and the
  bounded HTTP paths against loopback stubs this process owns (a dead port, a
  hung server, a body that never finishes, a cleartext server and a TLS-only
  server)
- `readme_coverage.rs` — the crate's own directories ship navigation docs

## Navigation
- [Repository Root](../../../README.md)
- [Parent: `runners/cli`](../README.md)
