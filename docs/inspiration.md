# Inspiration

Every private path this repository read, and what it taught. The private repositories are read-only
reference: nothing here was copied, vendored or transcribed. Where a value is quoted it is a single
identifier or a short configuration value, and it is quoted only because the lesson *is* the value
(a host literal that must not be repeated, a constant that should be named).

Two checkouts were read:

- the archived private engine (`superposition/qualia`, the interface reference this public tree was
  taken from), read in place;
- a read-only clone of `magic-cabinet/qualia`, used for its front ends and then discarded.

The lessons themselves — what each source does, what to keep, what to avoid — are in
[`frontend-lessons.md`](frontend-lessons.md). This file is the index of paths, so the reasoning is
discoverable without the private code.

## `superposition/qualia` (engine checkout)

- `runners/agent/src/main.rs` — inspected during the console recovery for its
  published HTTP contracts and the startup behavior of its Leash readers.
  A registered route and a working producer are separate acceptance checks.
- `scripts/windows-host.ps1`, `scripts/windows-host-runner.ps1` — inspected for
  the installed service's configuration and task lifecycle. The installed
  runner disables its Leash connection when the remote address is empty;
  changing a console address cannot repair that process's environment.
- Installed release `web/public/index.html` and `web/public/main.js` — inspected
  for the earlier operator controls, camera and spatial fields, belief sketch,
  arena events and observation request. Their implementations were not copied.

- `runners/watch/src/main.rs` — a ratatui panel and process supervisor in one binary: it spawns the
  runner set itself, reads `#[repr(C)]` state straight out of shared memory, and detects change by
  sequence number instead of by timer. Keep the supervisor and the tab-bar/content/status-bar split;
  avoid the two hard-coded runner lists and the provider-specific title badge.
- `runners/ops/src/main.rs` — egui/eframe is the right native stack for an operator panel, and HTTP
  belongs on a poller thread behind a command/message channel. Everything else about it is the
  counter-example: `DEFAULT_BASE_URL` is a `192.168.0.x` literal, discovery scans the local `/24`, and
  self-signed TLS is accepted by default.

## `magic-cabinet/qualia` — `apps/understanding-viewer`

- `apps/understanding-viewer/Cargo.toml` — the pure-Rust GUI stack, pinned exactly: `eframe`, `egui`
  and `egui_kittest` at `=0.33.3`. No webview, no JS runtime. This is the stack the console uses.
- `apps/understanding-viewer/README.md` — states the boundary in one line ("no HTML surface, embedded
  browser, HTTP server, or direct robot/device path") and documents `--check-stream` and `--bridge`,
  so the app can be exercised without a window and replayed without hardware.
- `apps/understanding-viewer/src/main.rs` — CLI shape for a viewer: exactly one of `--stream`,
  `--stdin` or `--bridge`, an explicit `eframe::Renderer::Wgpu`, and a `configure` function that sets
  the dark visuals and the minimum interaction size once.
- `apps/understanding-viewer/src/lib.rs` — the module boundary that matters: `app`, `model`,
  `profile`, `source`, `verification`, one concern each.
- `apps/understanding-viewer/src/app.rs` — a small view-state enum plus a width breakpoint:
  `LayoutMode::{Compact, Medium, Desktop}` and `CompactView::{Map, Layers, Timeline, Inspect,
  Profile}`. Keep the shape; the 1,276-line single file is what the console splits per view.
- `apps/understanding-viewer/src/model.rs` — the wire model and `FrameStore`, with a `validate()` on
  every frame and a bounded timeline. Validation is a property of the type, not of the renderer.
- `apps/understanding-viewer/src/source.rs` — the best pattern here: `StreamSource::{File, Stdin,
  Bridge}` behind a channel-publishing `SourceHandle`, so replay, pipe and live child process are the
  same to the view, and an unverified frame becomes an error event instead of a widget.
- `apps/understanding-viewer/src/verification.rs` — canonical JSON plus Ed25519 plus a
  `sha256:`-prefixed digest, checked before anything is drawn. Justified by the verified-frame
  requirement; too much machinery for a panel whose only input is one local agent response.
- `apps/understanding-viewer/src/profile.rs` — the shape for any local edit: original, draft, errors,
  and an `export` that refuses to write while validation fails and only ever writes the named path.
- `apps/understanding-viewer/tests/native_ui.rs` — the lesson the whole plan rests on: an
  `egui_kittest` harness stepped over a temp JSONL fixture, assertions by accessible label
  (`STOP`, `E-STOP`, touch targets), a frame-budget measurement, and `harness.try_snapshot(name)`
  collecting named image snapshots.
- `apps/understanding-viewer/tests/viewer.rs` — the model-level tests beside the UI ones: layout
  breakpoints, tamper rejection, bounded timeline, profile validation. Behaviour, not pixels.
- `apps/understanding-viewer/tests/support/mod.rs` — `FixtureState::{Healthy, Degraded,
  ProviderFailure, LocalizationLoss}` and a `frame_for(sequence, state)` builder: hand-authored
  fixtures so the degraded states are testable offline. The console's `braid-state.json` is this idea.
- `apps/understanding-viewer/tests/snapshots/desktop_degraded.png` — 1280×820 RGBA; a named degraded
  state committed as an image so a regression fails a test rather than needing eyes.
- `apps/understanding-viewer/tests/snapshots/desktop_provider_failure.png` — 1280×820 RGBA; the second
  of the three desktop failure states.
- `apps/understanding-viewer/tests/snapshots/desktop_localization_loss.png` — 1280×820 RGBA; the third.
- `apps/understanding-viewer/tests/snapshots/compact_healthy_replay.png` — 390×844 RGBA; the compact
  layout snapshot, which is also the touch-safety test's output.

## `magic-cabinet/qualia` — `apps/tui`

- `apps/tui/package.json` — an Ink terminal app whose dependency set (`ink`, `react`, `ssh2`, `ws`,
  `node-pty`) is the toolchain the console avoids, which ships no README and no tests, and whose
  thirteen scripts each launch a different root component. A tool a stranger cannot build or exercise
  is not a front end we want.
- `apps/tui/src/index.tsx` — the counter-example in one screen: the entire configuration is a source
  literal (`host: '192.168.0.221'`, a `homedir()` mount path), the app refuses to start if the SSHFS
  mount fails, and data arrives by `exec`ing shell commands. Config comes from the environment, never
  from source.
- `apps/tui/src/types/index.ts` — `type View = 'dashboard' | 'logs' | 'network' | 'agents'`: the same
  "one enum, one dispatch, one screen per value" shape as the Rust TUIs, from a third language.
- `apps/tui/src/lib/ssh-client.ts` — reads a private key from a fixed home-directory path and connects
  to a literal port. Credentials and ports are configuration.
- `apps/tui/src/lib/mount-manager.ts` — the read path goes through `sshfs`/`fusermount`/`mountpoint`
  run with `execSync`. A panel that needs a FUSE mount to render cannot run in a container or on CI.
- `apps/tui/src/lib/robot-data.ts` — the part worth keeping: a data-collection layer separated from the
  components, with the endpoint passed in rather than reached for from inside a view.
- `apps/tui/src/qualia-monitor.tsx` — a second, larger view set in the same app, and the address
  repeated as a constant inside the data function. One client, one configuration source.
- `apps/tui/src/multi-robot-index.tsx` — two addresses in a `ROBOT_HOSTS` array literal in source.
- `apps/tui/src/robot-control.tsx` — `ROBOT_URL` as a source literal; one of four screens that each
  re-declare the same endpoint.
- `apps/tui/src/robot-menu.tsx` — the endpoint repeated in the literal and again inside UI text.
- `apps/tui/src/robot-dashboard.tsx` — the same literal, a third time.
- `apps/tui/src/robot-compact.tsx` — the same literal, a fourth time.
- `apps/tui/src/rosbridge-monitor.tsx` — a hardcoded `ws://…:9090` for live sensor data, which is also
  why the web app (below) needs a WebSocket polyfill.
- `apps/tui/src/components/Dashboard.tsx` — a panel titled with the device's hardcoded address: chrome
  should name the configured endpoint, not the one the author had on their desk.
- `apps/tui/src/components/NetworkView.tsx` — prints the same address as a service URL.

## `magic-cabinet/qualia` — `packages/web`

- `packages/web/package.json` — 50 runtime plus 21 dev dependencies and a six-script toolchain for the
  same operator screens a single `cargo run -p qualia-console` renders. This is the measurement behind
  the plan's decision that a web SPA iterates more slowly than a native panel.
- `packages/web/vite.config.ts` — three build plugins beyond the bundler (router codegen, SWC, Tailwind
  v4), a personal ngrok hostname in `allowedHosts`, and a `ws` alias plus `global: 'globalThis'` that
  exist only because the device and the runtime are on opposite sides of the browser boundary.
- `packages/web/src/main.tsx` — five nested providers (query, web3, theme, font, direction) around the
  router, and 401/500 handling in a query cache. A local operator panel authenticates as whoever can
  reach the local agent's port; each of those layers is a place a snapshot test cannot reach.
- `packages/web/src/routeTree.gen.ts` — a committed code-generation output: the route tree is a build
  artifact tracked in git, so the inner loop of a UI change includes a generator step.
- `packages/web/src/routes/` — 33 tracked route files across auth, error and feature directories for a
  general-purpose dashboard (users, tasks, chats, bots, apps, settings). The console needs exactly five
  views.
- `packages/web/src/polyfills/ws.ts` — the browser-side stand-in for a robot WebSocket client; the
  clearest evidence that the front end and the device belonged in one runtime, not two.
- `packages/web/netlify.toml` — a static-host SPA redirect. Deployment surface the harness does not
  need, and one more thing to keep working.
