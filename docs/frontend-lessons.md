# Front-end lessons

Five front ends were read as evidence before `apps/qualia-console` was written. Four of them are
operator surfaces for the same kind of stack; one is a spatial inspector. None of their code is
copied: this document records what each one teaches, and
[`inspiration.md`](inspiration.md) records every private path that was read and what it taught.

The point of writing it down is that the new console is built *against this document*, so a design
decision in `apps/qualia-console` can be traced to the evidence that produced it instead of to taste.

## Sources, and where they were read

| # | Source | Repo | Checkout used |
| --- | --- | --- | --- |
| 1 | ratatui engine TUI | `superposition/qualia` (engine checkout) | `runners/watch/src/main.rs` |
| 2 | egui/eframe ops dashboard | `superposition/qualia` (engine checkout) | `runners/ops/src/main.rs` |
| 3 | native understanding viewer | `magic-cabinet/qualia` (private, read-only) | `apps/understanding-viewer/` |
| 4 | Ink terminal app | `magic-cabinet/qualia` (private, read-only) | `apps/tui/` |
| 5 | Vite + React SPA | `magic-cabinet/qualia` (private, read-only) | `packages/web/` |

Every path cited below is a real file that was opened. Where a file is in a private repository the
path is given as `<repo>/<path>`; the private repositories are inspiration only and no file was
copied from them.

### Path conventions

- **Evidence paths** are the `Source` and `Evidence` entries in each of the five sections below.
  Every one of them resolves when read, against the checkout named in the table above: engine paths
  under the engine checkout, `apps/understanding-viewer/…`, `apps/tui/…` and `packages/web/…` under
  the read-only `magic-cabinet/qualia` clone. A bare filename in prose (`app.rs`, `routeTree.gen.ts`)
  names a file in that section's source, not a path from this repository's root.
- `crates/…` and `runners/…` paths that are not in the table above are relative to **this**
  repository (`crates/shm`, `runners/init`, `runners/watch`).
- `apps/qualia-console/…` — and `tests/snapshots.rs` where it means the console's test file — names
  what Step 26 will build. It does not exist yet; it is a target, not evidence. This document is
  written before the crate exists so that the crate is built against it.

Nothing was dropped from this document: all five sources exist and were read. Source 2 (`runners/ops`)
is not carried in this repository — the plan's Step 27 retires the ops dashboard — so its section is
written as a lesson to carry forward rather than a component to maintain. If a source path had failed
to resolve, its section would have been deleted and the deletion stated here; that did not happen.

---

## 1. The engine TUI — `runners/watch/src/main.rs` (ratatui)

**What it does.** One binary that is both the supervisor and the panel. In supervisor mode
(`mode_str = "SUPERVISOR"`) `spawn_runners` resolves its own executable directory, starts
`NATIVE_RUNNER_NAMES` (`qualia-cuda-service`, `qualia-agent`, `qualia-lidar`, …) as child processes,
optionally adds `qualia-drive` behind `QUALIA_WATCH_WITH_DRIVE`/`WITH_DRIVE`/`QUALIA_DRIVE_ARMED` and
the legacy predictive-coding layers behind `QUALIA_WATCH_ENABLE_LEGACY`, and keeps them in a
`Vec<(String, Child)>` for shutdown. In monitor mode (`attach`) it reads a shared-memory segment by
name and renders it without spawning anything; the title bar always says which mode is live and how
many runners are attached (`shm:{name} | {n} runners`).

The panels read the `#[repr(C)]` ABI directly — `qualia_types::{BeliefSlot, LedgerEvent, NUM_LAYERS,
STATE_DIM, WEIGHT_COUNT}` — with no IPC layer between the renderer and the segment. `poll_ledger` and
`poll_thoughts` compare the segment's sequence numbers against the last ones rendered and return
early when nothing changed, so the redraw is driven by the data, not by a timer. The view is a small
enum (`ViewMode::{Overview, Detail, Hex, Sparklines, Residuals, Weights, World}`) with a `VIEW_LABELS`
table, digit keys `1`–`7`, Tab to cycle, a `match` dispatch, and one render function per variant. The
layout is three vertical constraints: a one-line tab bar, `Min(1)` content, a one-line status bar.

**Keep.**

- The supervisor. The panel that shows the stack is the program that starts it, so bring-up is one
  command and the operator cannot be looking at a different stack from the one running. This is what
  Step 27's Mission panel extends, and it is why `runners/init`'s manifest stays the single
  definition of the child set rather than a second copy in the TUI.
- The panel split: tab bar / content / status bar, one `ViewMode` enum, one label table, one `match`,
  one render function per panel. Adding the Mission panel is one enum variant, one label, one match
  arm and one function.
- Rendering the ABI straight out of shared memory with sequence-number change detection. The belief
  and Mission panels want exactly this, and `crates/shm` already defines the offsets.
- The environment allowlist in `spawn_runners`: the child gets `QUALIA_SHM_NAME`, `RUST_LOG` and a
  named list of keys, not the whole parent environment. A panel that supervises should not leak its
  own environment into the runners.
- Clean teardown: a signal handler flips an atomic, `shutdown_children` reaps the children, and
  `cleanup_stale_shm` unlinks the segment so the next start is not reading a dead stack.
- Mode and provenance in the chrome. `[SUPERVISOR]`/`[MONITOR]`, the shm name and the runner count
  are always visible; a Braid line should be equally explicit about which agent and generation it is
  showing.

**Avoid.**

- Two hard-coded runner lists in the binary (`NATIVE_RUNNER_NAMES` and `LEGACY_RUNNER_NAMES`) with a
  legacy opt-in flag. The public tree already has `QUALIA_STACK_MANIFEST`; the Mission panel must not
  reintroduce a second stack definition beside it.
- Provider-specific chrome. `has_gemini_key` and the `✓ GEMINI` badge are panel content for one
  vendor's key, which is neither engine state nor braid state.
- `expect()` on `current_exe()`/`parent()` in the supervisor path. If the executable path cannot be
  resolved, the panel that was supposed to supervise the stack panics instead of reporting it.
- One key binding per panel written twice (the `KeyCode::Char('1')` arm and `VIEW_LABELS`). One table
  should drive both the labels and the keys.
- The 20 fps poll loop and the 120-sample VFE history are presentation, not measurement. Journal
  numbers must come from an MCAP segment or a mage capture, never from a sparkline.
- `unsafe` byte-casting of `BeliefSlot` for the hex view. It is defensible in a read-only debug panel;
  the Mission panel needs no `unsafe` at all, and should have none.

**Evidence.** `runners/watch/src/main.rs` (engine checkout) — `ViewMode` and `VIEW_LABELS` at 65–83,
the key handler at 233–263, `spawn_runners` at 350, `env_flag` at 400, `ui` at 409, `shm_open` at 334,
`poll_ledger` at 1647, `poll_thoughts` at 1689. 1,797 lines.

---

## 2. The ops dashboard — `runners/ops/src/main.rs` (egui/eframe)

**What it does.** A native desktop dashboard, one window (460×620 default, 380×420 minimum), with a
tiny tab enum (`OpsTab::{Overview, World, Compute, Notes}`), a `render_tab_selector` and one `match`
in a scroll area. HTTP is done on a background thread: the UI sends `PollerCommand::{SetBaseUrl,
SetTlsMode, RefreshNow, StartExplore, StopExplore}` over a channel and receives `PollerMessage`s back,
so a slow or dead service degrades the status chips instead of freezing the window. The poll and retry
intervals are both 250 ms and each history series is capped at `HISTORY_CAPACITY = 180` samples. The
client is a blocking `reqwest` client with a 700 ms connect timeout and a 1200 ms total timeout.

The base URL came from the process environment, defaulting to the literal
`https://192.168.0.220:8080`. When polling fails, `auto_discover_base_url` derives the local `/24`
(`discover_local_subnet_base` opens a UDP socket to `1.1.1.1:80` and masks its own address,
`subnet_base`) or falls back to the configured host's network, then `scan_subnet_for_port` probes
addresses `.1`–`.254` with 24 worker threads at 120 ms each and accepts the first host whose
`GET /integration/status` deserializes.
`classify_error` turns transport failures into short human labels ("tcp connect refused", "tcp host
unreachable", "tcp connect timeout"). Window position and size persist to
`dirs::config_dir()/qualia/qualia-ops-window.json`, outside the repository.

**Keep.**

- The stack. `egui` + `eframe`, native, one binary, no webview and no JavaScript runtime. This is the
  stack the console uses, and it is why the five views are modules in one Rust crate rather than a
  page tree.
- The background poller. The command/message channel pair keeps HTTP off the UI thread and turns a
  dead service into a visible status rather than a hang. The console's `GET /braid` polling should
  have the same shape.
- Tab enum + selector function + one `match`. The same lesson as source 1, arrived at independently;
  that agreement is the strongest evidence for it.
- Named cadence and capacity constants (`POLL_INTERVAL`, `RETRY_INTERVAL`, `HISTORY_CAPACITY`) rather
  than numbers scattered through the render code.
- Error classification that a human can act on, and window state in the platform config directory
  instead of a file in the working tree.

**Avoid.**

- The `192.168.0.x` assumption. `DEFAULT_BASE_URL = "https://192.168.0.220:8080"` and a `/24` scan are
  a household's subnet baked into a shipped binary. The console reads `QUALIA_AGENT_URL` with a
  `http://127.0.0.1:8080` default and nothing else; `grep -rn "192\.168\." apps/qualia-console/src`
  must return nothing.
- Subnet autodiscovery as a feature. A 254-address probe is indistinguishable from a scan, ambiguous
  when two agents answer, and impossible to drive from the committed fixture the snapshot tests use.
  Probe the one configured URL and render its failure.
- `danger_accept_invalid_certs(true)` as the default — both `RemoteState::default` and the poller
  start there. The console talks cleartext to `127.0.0.1`; if TLS is ever introduced it is explicit
  and off by default.
- Growth by accretion: 2,274 lines in one `main.rs`, holding transport, discovery, window-state
  persistence, dozens of response `struct`s and every panel's drawing code. The console puts one view
  per module under `src/views/`.
- Deserializing a service's whole response vocabulary (`WorldSnapshotResponse`, `CudaInfo`,
  `PlannerSummary`, …) in the same file as the painter. Wire types belong with the client, not the
  widget.
- Reaching for a discovery convenience before the failure path is legible. A console that shows
  "agent unreachable at <url>" is more useful than one that scanned a subnet and found a different
  machine.

**Evidence.** `runners/ops/src/main.rs` (engine checkout) — `DEFAULT_BASE_URL` at 12, `main` and
window state at 17–40, the base-URL environment variable at 56, `OpsTab` at 809, `build_client` at 988,
`auto_discover_base_url` at 1641, `discover_local_subnet_base` at 1663, `subnet_base` at 1672,
`scan_subnet_for_port` at 1677, `window_state_path` at 1710. 2,274 lines. Retired by Step 27; kept as
a lesson.

---

## 3. The native understanding viewer — `magic-cabinet/qualia/apps/understanding-viewer`

**What it does.** A native `eframe`/`egui` desktop application (`eframe`, `egui` and `egui_kittest`
pinned to `=0.33.3`, `eframe::Renderer::Wgpu`, minimum window 360×640) that renders a signed,
validated viewer stream. Its README states the boundary plainly: "no HTML surface, embedded browser,
HTTP server, or direct robot/device path".

The shape is three pieces and each one is separable:

*A mode and a view enum.* `layout_mode(width)` maps the content-rect width to
`LayoutMode::{Compact, Medium, Desktop}`; Desktop and Medium arrange egui side/bottom panels, and
Compact switches on a five-variant `CompactView::{Map, Layers, Timeline, Inspect, Profile}` rendered
under a bottom rail. The same data, three arrangements, one enumeration — and the compact rail is
driven by the same values the tests click by label.

*A source behind a channel.* `StreamSource::{File, Stdin, Bridge { program, args }}` is turned into a
`SourceHandle { events: Receiver<StreamEvent>, controls: Option<Sender<ControlRequest>> }`. The file
and stdin sources are pure replay; the bridge spawns a child process and writes control requests to
its stdin as JSONL, so stop and E-stop travel back over the same private pipe. Crucially the reader
thread validates every frame (`frame.validate()`) and every control result before publishing them, and
converts a failure into `StreamEvent::Error` — the view never receives an unverified frame.

*Verification before display.* `verification.rs` canonicalizes JSON (keys sorted, `signature`
removed), signs and verifies with Ed25519 (`ed25519-dalek`), and `verify_session` checks that the
identity manifest, the session provenance and the frame's own session/entity/profile all agree and
that the canonical digest matches. Profile editing is local and explicit: `ProfileDraft` holds
`original`, `draft`, `errors` and an optional export path, `changed_line_count()` computes the diff,
and `export()` refuses to write while any validation error stands and writes only the path the user
named.

*The tests are the reason to read this one.* `tests/native_ui.rs` builds an `egui_kittest::Harness`
with `.wgpu()` and `build_eframe`, points the app at a temp JSONL file holding one fixture frame, and
steps it in a bounded loop (8 iterations, 2 ms apart) until a status label (`READY`, `DEGRADED`,
`FAILED`) appears. Assertions are by accessible label — `query_by_label("STOP")`,
`query_by_label("E-STOP")` — and by measured touch targets (`TOUCH_TARGET >= 48.0`,
`ESTOP_TARGET >= 56.0`). One test asserts a frame budget: 30 steps of a 10,000-cell frame must average
under 16.7 ms. Four snapshots are committed as PNGs: `desktop_degraded`, `desktop_provider_failure`
and `desktop_localization_loss` at 1280×820 RGBA, and `compact_healthy_replay` at 390×844 RGBA — three
degraded desktop states and one compact healthy state, with `SnapshotResults::add(harness.try_snapshot(name))`
collecting them. The fixtures come from `tests/support/mod.rs`: a `FixtureState::{Healthy, Degraded,
ProviderFailure, LocalizationLoss}` enum and a `frame_for(sequence, state)` builder, all hand-authored,
so no test needs a device, a network or a live stack.

**Keep.**

- The lesson the plan names: a small view-state enum plus **image snapshots of named degraded states**,
  driven from committed fixtures, so a UI regression fails a test instead of needing eyes. The console
  ships `tests/snapshots.rs` with `mission_healthy`, `mission_degraded`, `belief_stale` and
  `evidence_empty`, driven by the committed `apps/qualia-console/tests/fixtures/braid-state.json`, and
  written for the empty and error cases first — the four snapshots here are three degraded states plus
  one compact healthy replay, which is the right weighting.
- Accessibility labels as the assertion surface, plus minimum touch targets. It is what makes the
  snapshot test a behavioural test rather than pixel worship, and it costs nothing to keep.
- Three arrangements of one view state (`LayoutMode`) rather than three layouts with three code paths;
  the console's five views should be one enum with one `match`.
- Validation before display, behind a channel, with the source abstract (file, stdin, bridge). The
  console's braid fixture is the file source, a live agent is the bridge source, and the view should
  not know which one it has.
- The draft-editing shape (`original` / `draft` / `errors` / explicit `export`) for any local edit,
  with export refusing invalid state. Nothing in this app writes to git.
- Pinning the GUI crates exactly (`=0.33.3`) in a workspace that builds on one machine and deploys to
  another.

**Avoid.**

- `.wgpu()` snapshots assume a GPU-capable test runner. That is affordable here because development
  and verification run on the 4090, but it must be stated, not discovered; keep the wall-clock frame
  budget test (`< 16.7 ms`) separate from the deterministic snapshot set, since a timing assertion is
  environment-dependent and will flake on a loaded machine.
- The single 1,276-line `app.rs` mixing layout, health colours, drawing primitives, the profile editor
  and value summaries. The console splits one module per view.
- The panel set itself. A voxel/scan/path/counterfactual inspector is the right shape for a spatial
  replay and the wrong one for an operator console; borrow the structure, not the screens.
- `include_str!`-embedded schemas, a hand-rolled canonicalizer and a bespoke Ed25519 envelope are
  justified by the "verified frame" requirement here and would be weight in a panel whose only input
  is one JSON object from the local agent.
- Three desktop snapshot files at ~185 KB each plus a 37 KB compact one, all in git. The console's
  fixtures are text; the snapshots should be small and few.

**Evidence.** `magic-cabinet/qualia` — `apps/understanding-viewer/Cargo.toml` (eframe/egui/
egui_kittest `=0.33.3`), `apps/understanding-viewer/README.md` (no-HTML boundary, `--check-stream`,
`--bridge` stop/E-stop), `src/main.rs` (`Renderer::Wgpu`, 360×640 minimum), `src/app.rs`
(`LayoutMode` 35, `CompactView` 52, `UnderstandingApp` 122, `update` 462, `health_color` 577),
`src/model.rs` (`FrameStore`, `ViewerFrame`), `src/source.rs` (`StreamSource`, validation in
`read_lines`), `src/verification.rs` (`canonicalize`, `verify_session`), `src/profile.rs`
(`ProfileDraft`), `tests/native_ui.rs` (`viewer_harness_for_frame` 20, `compact_replay_is_accessible_and_touch_safe` 69, `desktop_snapshots_cover_degraded_provider_and_localization_states` 124),
`tests/viewer.rs`, `tests/support/mod.rs` (`FixtureState`), `tests/snapshots/{desktop_degraded,desktop_provider_failure,desktop_localization_loss,compact_healthy_replay}.png`.

---

## 4. The Ink terminal app — `magic-cabinet/qualia/apps/tui`

**What it does.** A TypeScript/React terminal application (Ink 6, React 19, `ssh2`, `ws`, `node-pty`)
that monitors and drives a Jetson over SSH. The top-level app declares
`type View = 'dashboard' | 'logs' | 'network' | 'agents'` and renders one component per value; a second
826-line screen, `qualia-monitor.tsx`, declares its own, larger view set
(`'services' | 'agents' | 'logs' | 'ipfs' | 'blockchain' | 'chat'`) with six sibling components; and a
third family (`robot-menu`, `robot-control`, `robot-dashboard`, `robot-compact`, `rosbridge-monitor`,
`multi-robot-index`) re-implements robot control screens on top of the same endpoint.

Data does not arrive over a protocol. `SSHClient` opens a raw SSH connection and `exec`s shell
commands, returning stdout+stderr as a string; `MountManager` creates an SSHFS mount with `execSync`
and reads go through `${mountPath}/home/${username}/…`; live sensor data comes from `ws://…:9090`
(rosbridge). The connection key is read from `~/.ssh/id_ed25519` and the port is a literal `22`.

Configuration does not exist. The `RobotConfig` object is a source literal in `src/index.tsx`:
`host: '192.168.0.221'`, `username: 'jetson'`, a mount path under `homedir()`. Every other screen
repeats the address — `ROBOT_URL = "http://192.168.0.221:5000"` in four robot components,
`ROBOT_HOSTS = ['192.168.0.220', '192.168.0.221']` in `multi-robot-index.tsx`,
`ROBOT_IP = '192.168.0.221'` in `qualia-monitor.tsx`, `ws://192.168.0.221:9090` in
`rosbridge-monitor.tsx`, and the address printed into the UI text itself. Eleven `node_modules/…`
paths are tracked in git. The package has no README and no tests, and its thirteen scripts each launch
a different root component (`start`, `robot`, `robot:menu`, `monitor`, `rosbridge`, …) — the screens
were never a single application.

**Keep.**

- The view list is small and flat at the top level (four views), and each view is one component. Same
  lesson as sources 1–3, from a third language: one enum, one dispatch, one screen per value.
- Separating a data layer (`lib/robot-data.ts`) from the rendering components. The console does this
  with one client module against `GET /braid` and one fixture-backed fake behind the same interface.

**Avoid — this is the counter-example the plan names.**

- No host, token, key path, port or mount may appear in source. This app hardcodes all five: the host
  is repeated in twelve files under `apps/tui/src`, the key path is `~/.ssh/id_ed25519`, the port is a
  literal `22`, and the mount path is a `homedir()` string. The console reads `QUALIA_AGENT_URL`
  (default `http://127.0.0.1:8080`) and
  prints that value in its status line if it wants the operator to know where it is pointing;
  `grep -rn "192\.168\." apps/qualia-console/src` is the test, and it must return nothing.
- Do not put a mount in the read path. `MountManager` shells out to `sshfs`, `fusermount` and
  `mountpoint` through `execSync`, and the app refuses to start when the mount fails. An operator
  panel that needs a FUSE mount in order to render is a panel that cannot be run from a container, a
  CI job or the deployment target.
- Do not vendor `node_modules` into the repository.
- Do not let four screen families each re-declare the same endpoint. One client, one configuration
  source, one place an address appears.
- Do not ship a tool with no README and no tests: a stranger cannot build it, and no view can be
  exercised without the hardware it was written against. The thirteen scripts here — one per root
  component — are what a view set looks like before it is unified behind one entry point. The console
  ships `tests/snapshots.rs` from its first commit for exactly this reason.

**Evidence.** `magic-cabinet/qualia` — `apps/tui/package.json` (`@qusd/tui`, ink/react/ssh2/ws/
node-pty), `src/index.tsx` (`RobotConfig` literal 16–20, view state and key bindings),
`src/types/index.ts` (`View`), `src/lib/ssh-client.ts` (host from config, key path, port 22),
`src/lib/mount-manager.ts` (`sshfs`/`fusermount`/`mountpoint` via `execSync`),
`src/lib/robot-data.ts` (endpoint string), `src/qualia-monitor.tsx` (`View` at 15, `ROBOT_IP` at 269),
`src/multi-robot-index.tsx` (`ROBOT_HOSTS` at 26), `src/robot-control.tsx` (`ROBOT_URL` at 12),
`src/robot-menu.tsx`, `src/robot-dashboard.tsx`, `src/robot-compact.tsx`, `src/rosbridge-monitor.tsx`,
`src/components/{Dashboard,NetworkView}.tsx`.

---

## 5. The Vite + React SPA — `magic-cabinet/qualia/packages/web`

**What it does.** A general-purpose operator web dashboard: Vite with `@vitejs/plugin-react-swc`,
`@tailwindcss/vite` and the TanStack Router plugin (which generates `routeTree.gen.ts`, and yes, the
generated file is committed), React 19, TanStack Query/Router/Table, Radix UI, Clerk authentication,
axios, a Web3 provider, 50 runtime dependencies plus 21 dev dependencies (71 pinned packages) and 33
tracked route files spread over
`routes/(auth)`, `routes/(errors)`, `routes/_authenticated` and feature directories (`apps`, `bots`,
`chats`, `control`, `dashboard`, `settings`, `tasks`, `users`). `src/main.tsx` nests five providers
(`QueryClientProvider` → `Web3Provider` → `ThemeProvider` → `FontProvider` → `DirectionProvider`) plus
`RouterProvider`, installs a query cache that redirects on 401 and navigates to `/500` on 500, and
imports the generated route tree. `vite.config.ts` carries a `server.allowedHosts` entry for an ngrok
tunnel, a `ws` polyfill alias, and a `global: 'globalThis'` `define`; the build is
`tsc -b && vite build`, with eslint, prettier and knip beside it, and a `netlify.toml` for deployment.

**What it establishes.** This is the evidence behind the plan's decision that a web SPA plus its
toolchain iterates more slowly than a native Rust panel for the same operator screens. The operative
facts are the ratio, not the taste: 50 runtime dependencies plus 21 dev ones, a code-generation step
whose output is a committed file, three build plugins beyond the bundler, eslint + prettier + knip on
top, and 33 route files — to render a set of screens a single Rust binary renders from one
shared-memory segment. Adding one operator panel to a native `egui` app is one function and one enum
arm; the equivalent here is a route file, a component, possibly a query hook and an entry in an auth
layout, and the regenerated route tree.

**Keep.**

- The scoping. The console needs the operator screens — mission, belief, world, evidence, telemetry —
  and not a users/tasks/chats/apps/bots dashboard; five views in one crate is the direct application
  of this.
- Keeping the runtime beside the stack. This SPA needs a synthetic `ws` module, `globalThis` shims and
  a browser polyfill tree only because the runtime and the device are on opposite sides of a browser
  boundary. A native panel talks to the local agent directly and needs none of that.
- Keeping the panel unauthenticated. Five providers are five places a screenshot test cannot reach and
  five failure modes an operator cannot diagnose; the console authenticates as "whoever can read the
  local agent's port", and `QUALIA_AGENT_URL` is the whole configuration surface.

**Avoid.**

- The toolchain. A router codegen step, Tailwind, SWC, eslint + prettier + knip and 71 pinned
  packages is a second build system to keep green in a repository whose other half is one
  `cargo build`. It is also a second supply chain: every one of those packages is a constraint on the
  deployment target.
- Deployment scaffolding the harness has no use for (`netlify.toml`) and tunnel configuration in the
  dev-server config (`allowedHosts: ['thescoho.ngrok.app']`). Neither `192.168.0.x` nor a personal
  tunnel hostname belongs in a shipped front end; the console's only address knob is
  `QUALIA_AGENT_URL`.
- A dependency-graph build (codegen → route tree → type-check → bundle) as the inner loop of UI work
  on a machine whose GPU should be running beliefs. `cargo run -p qualia-console` is the inner loop
  here.

**Evidence.** `magic-cabinet/qualia` — `packages/web/package.json` (50 + 21 dependencies; `dev`/`build`/
`lint`/`preview`/`format`/`knip` scripts), `packages/web/vite.config.ts` (three plugins,
`allowedHosts`, `ws` alias, `define.global`), `packages/web/src/main.tsx` (provider stack,
`routeTree.gen` import, 401/500 handling), `packages/web/src/routeTree.gen.ts` (committed generated
file), `packages/web/src/routes/` (33 tracked files), `packages/web/src/polyfills/`,
`packages/web/netlify.toml`.

---

## What this means for `apps/qualia-console`

|Decision|Comes from|
| --- | --- |
| `egui` + `eframe`, one native binary, no webview, no JS runtime|Sources 2, 3 (stack), 4, 5 (what it avoids)|
| Five views, one enum, one module per view under `src/views/`|Sources 1, 2, 3, 4|
| `QUALIA_AGENT_URL` is the only address in the source; default `http://127.0.0.1:8080`|Sources 2, 4 (counter-example)|
| No subnet autodiscovery, no host literals, no TLS-insecure default|Source 2|
| Snapshot tests for `mission_healthy`, `mission_degraded`, `belief_stale`, `evidence_empty`, driven by the committed `braid-state.json`, empty and error cases first|Source 3|
| Assert by accessible label and by measured targets, not by pixels alone|Source 3|
| Poll `GET /braid` off the UI thread through a command/message channel|Source 2|
| The status line names the URL, the agent and the generation it is showing|Sources 1, 2, 4|
| No `unsafe` in the console|Source 1|
