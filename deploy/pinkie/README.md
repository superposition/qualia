# `deploy/pinkie` — the board mission run (T29, EPIC-10 step 29)

Pinkie is the Waveshare-carried Jetson Orin NX at `jetson@192.168.55.1` that carries the robot's
software — Leash on `:8000`, llama.cpp on `:8080` ([`docs/decisions.md`](../../docs/decisions.md)
D-010). Step 29 is the board half of the end-to-end run:

> Rebuild the fatbin per Step 18 (`CUDAARCHS=87-real`), copy the prior directory, run the same
> zero-motion stack with `QUALIA_CUDA_SM=87`, and assert `nvidia-smi` reports under 8 GB used and
> that `GET /braid` reaches the same terminal state.

Two files run it, and a third is the ticket's own smoke path:

| File | Side | What it is |
| --- | --- | --- |
| `ship-mission.sh` | dev host | Stages the head as a `git archive` and scp's it to the board. |
| `run-mission.sh` | board | Preflight, rebuild, run the zero-motion stack, drive the mission, assert, stop. |
| `scripts/mission_check.py` | both | Step 29's two assertions; `--self-test` runs its fixtures on the host. |

## Running it

Host — stage, read the commands, then ship:

```bash
bash deploy/pinkie/ship-mission.sh --stage-only      # writes the tarball, sends nothing
bash deploy/pinkie/ship-mission.sh                   # same, then scp to the board
```

The tarball travels as `git archive` output, never a working copy: a Windows checkout writes CRLF,
and CRLF breaks shell scripts and the cross-build Dockerfile on Linux (D-010). `ship-mission.sh`
checks every shipped script, Makefile and Dockerfile in the archive for a CR — one missed file is the
failure the guard exists for — and refuses if it finds one; other shipped files that carry CRLF (the
Mermaid sources, a JSON export) are listed, not refused, because the parsers the board uses accept
them.

Board — the invocation Main runs (the defaults are already the ticket's):

```bash
ssh -i ~/.ssh/qualia_jetson_ed25519 jetson@192.168.55.1
mkdir -p ~/qualia-board/<short> ~/qualia-board/runs
tar xzf ~/qualia-board/qualia-<short>.tar.gz -C ~/qualia-board/<short> --strip-components=1
cd ~/qualia-board/<short>
bash deploy/pinkie/run-mission.sh --plan    # optional: print every command first
bash deploy/pinkie/run-mission.sh
```

`run-mission.sh --plan` prints the build, run, assert and stop commands without executing any of
them; `--check` runs the preflight only. A pass ends with `mission: PASS` and exits 0.

## The two assertions

`scripts/mission_check.py` is the ticket's smoke path. It delivers one bounded exploration mission to
the agent's broker — `POST /mission-control/envelopes`, `start` then `cancel`, `MissionBroker` bearer
from `QUALIA_MISSION_BROKER_TOKEN` — and judges:

- **braid-terminal** — `GET /braid`'s `open_missions` rises by one and returns to where it started,
  and the mission record behind it reaches a terminal status. The starting count is read before the
  delivery, so a repeated run on a stack that already holds missions still asserts the transition.
  The mission is held open for `--hold-s` seconds (3 by default) before the cancel, so the memory
  leg below has a window to sample rather than one instant.
- **gpu-memory** — the peak memory over the run stays under the budget (8 GiB by default). The
  reading comes from `nvidia-smi` first; a Jetson reports `[N/A]` for its memory counters, so
  `tegrastats` — the board's own sampler — is the documented second source. When neither produces a
  number, the leg reports `CANNOT-ASSERT` and exits 2: an unjudged run is not a pass.

A pass prints (shape, not a measurement — the numbers are the board's to produce):

```text
mission-check: agent https://127.0.0.1:8081 answered; braid before: open_missions=0 session=<id> generation=0
mission-check: mission t29-board-<stamp> accepted (HTTP 202, idempotent_replay=False)
mission-check: mission t29-board-<stamp> cancel answered HTTP 200
mission-check: mission record terminal: status=cancelled stage=terminal code=cancelled detail=broker cancelled the mission
mission-check: braid-terminal OK (open_missions 1 -> 0 (started at 0; rose 0.0 s in, returned 3.7 s in); session=<id> generation=0)
mission-check: gpu-memory OK (peak <n> MiB of 8192 MiB budget over <m> sample(s) from nvidia-smi)
mission: OK
```

The checker's own fixtures run anywhere, with no board and no stack:
`python3 scripts/mission_check.py --self-test` prints `mission-check: self-test OK (40 cases)` and
covers the transition's boundaries, the 8192 MiB bound, both memory parsers, the mission-record shapes
the agent answers with (through the real lookup, so a revert to the flat-only match fails), and the
envelope bounds the broker validates.

## Board facts the runbook assumes

Every one of these is measured, not assumed (D-010, D-016, D-018; the T29 smoke on issue #45):

- **No DNS.** `git clone` and crates.io are unreachable there, so the tree ships as a tarball and the
  build is offline. The crate cache on the board is a legacy artifact: it is populated but partial,
  and its registry index may not match the shipped `Cargo.lock`.
- **The dev host cannot cross-build for the board** (D-018): the Docker engine is unreachable,
  `cross` is not installed, and there is no aarch64 gcc. The working lane is a native aarch64 build
  on the board, which is what `run-mission.sh` does — ~30 s for a subset, bounded to `-j 2`.
- **Ports.** `22` ssh, `53` resolved, `8000` Leash, `8080` llama.cpp. The agent therefore runs on
  `QUALIA_WEB_PORT=8081` (the smoke measured `9000`, `8100` and `8081` free); the run keeps its own
  compute socket, journal, session store, TLS directory and MCAP root under `--run-dir`.
- **RAM.** The board shows ~3.6 GiB total, so the 8 GB bound cannot be the binding constraint; the
  number the sampler records is the evidence, and the run failing to complete is the real failure.
- **Clock.** The board's clock runs ~10 days behind the host, which is why nothing here depends on
  TLS to a remote peer or on wall-clock agreement.
- **The manifest's `env` block wins over the shell.** `run-mission.sh` therefore exports only keys
  the manifest lists in a runner's `env_passthrough` (the supervisor applies those after the stack
  env): `QUALIA_CUDA_SM` for the compute service, the fly mode and prior path for the belief and
  explore paths, and the agent's port, token, journal, store, TLS directory, MCAP root and arena
  session. If the manifest's runner set or passthrough lists change, those exports are what must move
  with them.
- **The prior** ships inside the archive at `assets/brain/prior` (`graph.bin`, `manifest.json`,
  `attribution.json`). A deployment-scale prior built off the Male CNS dataset is external and is
  shipped alongside with `ship-mission.sh --prior DIR`, which names it in the printed invocation.

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | Both assertions hold (`mission: PASS`). |
| 1 | An assertion failed, or the mission could not be driven at all. |
| 2 | Preflight refused, `--check` was asked for, or the checker could not judge a leg (it exits 2 and the run passes that code through: `mission: CANNOT-ASSERT`). |
| 3 | The build failed. |
| 4 | The stack did not start or did not stop. |

## If the board build fails on resolution

The smoke hit `error: no matching package named ... found / location searched: crates.io index` on a
board whose registry index predates the shipped lock. Its measured remedies, in order:

```bash
# the board's cargo is rustup-local, not on PATH
export PATH=$HOME/.cargo/bin:$PATH
# if the lock's versions are not cached, re-lock against what is:
cargo generate-lockfile --offline
# if a crate is simply absent from the cache, ship its closure from the host
# (the smoke shipped turso's 103 .crate files by hand — D-016), or `cargo vendor`
# on the host and rsync the result (the smoke's §6.7).
```

If the run dies from memory pressure rather than a build error, Step 29's own remedy is the one to
take: drop the perception kernel's voxel grid resolution by half in
[`kernels/perception_voxel.cu`](../../kernels/perception_voxel.cu), re-run Step 18's budget test
(ticket #33), and **do not** reduce the belief or action kernels.

## What this does not do

- No motor path: the ticket's zero-motion manifest opens none, and Leash is only addressed by URL.
- No cross-build (D-018), no mage capture, no profiling — the memory leg is a sampler, not a profile.
- Nothing on the board is written outside `~/qualia-board/` and the run directory; no apt, no service
  change, no restart of the running Leash or llama.cpp.

## Evidence

`run-mission.sh` writes into `--run-dir` (default `~/qualia-board/runs/<UTC stamp>`):

| File | Contents |
| --- | --- |
| `evidence.txt` | Everything the run printed, including the preflight, the build, the assertions and the stop. |
| `mission.json` | The checker's observation record: the braid samples, the mission record, the memory samples. |
| `build.log` | The `make` and `cargo` output. |
| `stack.out` | The supervisor's own output. |
| `logs/` | One log per runner, as `qualia-init` writes them. |
| `mcap/` | Whatever the arena recorder sealed when the stack stopped. |

Copy it back with the line the run prints:

```bash
scp -i ~/.ssh/qualia_jetson_ed25519 jetson@192.168.55.1:~/qualia-board/runs/<stamp>/evidence.txt .
```

The handoff comment quotes that file's build/deploy commands and the output observed on the board,
per [`docs/agents.md`](../../docs/agents.md) §Definition of done.
