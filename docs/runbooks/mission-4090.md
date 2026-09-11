# The 4090 end-to-end mission (Step 28)

Step 28 of [EPIC-10](https://github.com/superposition/qualia/issues/15), ticket
[T28](https://github.com/superposition/qualia/issues/44). It is a **manual run**, not a unit test:
the driver is `scripts/mission-4090.ps1`, the stack is the repository's own supervisor and manifest,
and the checks it prints are the four facts Step 28 names.

## What it asserts

With the connectome prior built by Step 8 (`assets/brain/prior/`) and `QUALIA_FLY_MODE=prior`, the
zero-motion stack runs in simulation and one exploration mission is opened and closed:

| # | Observable | How the driver checks it |
| --- | --- | --- |
| 1 | `GET /braid` shows `open_missions: 1` then `0` | polls the agent's `/braid` after the `start` and `cancel` envelopes |
| 2 | a sealed MCAP segment exists | `qualia-mcap-inspect <mission-id>.mcap` replays the sealed file, and no `*.partial` is left |
| 3 | `qualia-session` lists one session with the mission id | `qualia-session list` / `show` after the recorder attached the sealed reference |
| 4 | the console's Mission view shows the same id; the watch Braid line shows the same generation | both front ends render `GET /braid`; the driver captures the braid view and checks `session_id` is the mission id |

The front ends are interactive (an `egui` window and a `crossterm` panel), so the driver proves the
shared state instead of raking their pixels: the console's Mission view and the watch Braid line are
tested to render `session_id` and `generation` from that view
(`apps/qualia-console/tests/snapshots.rs` `mission_healthy`, `runners/watch/tests/braid.rs`), and the
driver checks the view they read. The visual confirmation is the manual step below.

## Prerequisites

- The Step 8 artifact is committed: `assets/brain/prior/{graph.bin,manifest.json,attribution.json}`.
- The stack binaries are built (`target/release/`): the driver builds
  `qualia-cli qualia-init qualia-agent qualia-explore qualia-arena-recorder qualia-session qualia-mcap
  qualia-health qualia-cuda-service` with `cargo build --release -j 2`, one build at a time
  (`docs/agents.md` §Host safety, D-014). `-SkipBuild` reuses an existing build.
- **The compute service opens a CUDA context, so the run is GPU work.** Announce it and take the
  serial GPU window before starting (D-011); no other CUDA job may run while the stack is up.
- `nvidia-smi` must report the device; the run sets `QUALIA_CUDA_SM=89` for the 4090 (the board's
  Step 29 uses `87`).

## Running it

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/mission-4090.ps1 -MissionId explore-frontier-1
```

The driver:

1. pre-creates the session store row for `explore-frontier-1` (the MCAP path the recorder will seal);
2. starts the stack with `qualia run --manifest config/stack-manifest.zero-motion.json` — the same
   `qualia-init` every deployment uses, reading the same manifest (`QUALIA_SHM_NAME`,
   `QUALIA_SOCK_PATH`, `QUALIA_LOG_DIR` come from the manifest and the driver's scratch overrides);
3. waits for `GET /braid`, then delivers `POST /mission-control/envelopes` with a
   `qualia.mission-envelope.v1` body: `command: start`, `objective.kind: explore_frontier`,
   producer epoch 1 sequence 1, and an evidence reference (a `start` requires one);
4. fills in the missing subsystem honestly: the evidence-grounded planner is not in this build, so
   the mission parks at `awaiting_fresh_evidence` and the second envelope (`command: cancel`,
   sequence 2) ends it with the broker's own `stop_verified` — it never entered motion;
5. waits for the recorder's bounded run (`QUALIA_MCAP_DURATION_SECONDS`) to seal the segment: on
   Windows `qualia stop` terminates the supervisor's children hard, so the graceful seal is the
   recorder's own duration stop (on Linux the shutdown signal seals it as well), and then stops
   the stack;
6. runs the three checks and prints an `evidence.json` under `-ScratchRoot`.

Configuration is the environment the manifest does not fix: the broker token
(`QUALIA_MISSION_BROKER_TOKEN`; loopback does **not** bypass the mission-broker scope), the scratch
store/journal/TLS/MCAP roots, a unique `QUALIA_COMPUTE_SOCKET`, and
`QUALIA_ARENA_SESSION=<mission-id>` so the segment, the store row and the braid's session all carry
the mission id. The manifest's `env` block pins `QUALIA_FLY_MODE=prior`,
`QUALIA_FLY_PRIOR_PATH=assets/brain/prior` and `QUALIA_CUDA_SM=89`; every key an operator may
override for another deployment is listed in that runner's `env_passthrough`, so the board sets
`QUALIA_CUDA_SM=87` without editing the manifest.

## The manual front-end step

With the stack running (`-KeepRunning -HoldOpenSeconds 30` keeps the mission open, then leaves the
stack up after it closes), in another terminal:

```powershell
$env:QUALIA_AGENT_URL = 'https://127.0.0.1:18443'
cargo run -p qualia-console          # Mission window: session = the mission id, open missions = 1
cargo run -p qualia-watch -- --attach  # Mission panel and Braid line: "braid gen 0 · belief lag: N ms"
```

Both read the same `GET /braid` the driver captured, so the window and the panel show the mission id
and generation the checks already assert.

## Honest limits

- The mission parks at `awaiting_fresh_evidence` in this build; no plan is produced and the robot
  never moves. The zero stop is the broker's, and the sealed MCAP is a zero-motion history — that is
  what Step 28 asks for, not a drove mission.
- The GUI and the TUI cannot be confirmed headlessly; their rendering of `session_id`/`generation` is
  pinned by their own tests and the manual step above is what closes the loop.
- The aarch64 build and the board run are Step 29 (T29): a native build on Pinkie from a `git archive`
  of the head (D-018 — the host cannot cross-build), then the same driver with
  `QUALIA_CUDA_SM=87` and the board's ports.
