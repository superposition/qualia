# Decisions

Recorded so the tracker and the code cannot drift from the reasoning.

## D-001 — Clean-room rewrite

The public repository is a from-scratch rewrite. **No file is copied from any private
repository.** The private engine is read as a reference for its *interfaces* only (crate names,
public type and function signatures, wire contracts, SHM layout); every line in this repository is
written here.

Consequences:

- The crate names and public APIs match the private engine, so the plan's steps remain valid.
- `superposition/qualia-private` is archived and read-only. It is never a build input and never a
  publication source; its history does not ship.
- The front end is rebuilt from the recorded lessons in [`frontend-lessons.md`](frontend-lessons.md),
  not copied from any of the four existing front ends.
- The connectome dataset is consumed as data (CC-BY, attributed in `NOTICE`), never as code.

## D-002 — Licence

Apache-2.0 with a `NOTICE` carrying the Leash MIT notice and the Male CNS CC-BY attribution. Guarded
by `.github/scripts/notice-check.sh`.

## D-004 — Dependency pins forced by the toolchain

rustc 1.94.1 rejects two crates the plan's manifests resolve to, so both are pinned:

| Crate | Pinned | Why |
| --- | --- | --- |
| `owo-colors` | `4.3.0` | 4.4.0 fails const-eval (`E0080`) on 1.94.1, which breaks every `mcap`-dependent crate. |
| `arrow` | `56` | `arrow-arith` 53.4.0 declares an internal `ChronoDateExt::quarter()` that collides with `chrono::Datelike::quarter` (chrono 0.4.44+), so arrow 53 cannot build here. 56 is already the workspace's arrow via `rerun`, with the same IPC reader. |

Pins live in `Cargo.lock`, with the reasoning in a comment in the crate manifest that needs it.

## D-005 — `edge_count` is CSR nonzeros

The plan's Verification 2 expects the builder to print `prior: 5 types, 12 edges` for a fixture with
12 segment rows. Those 12 rows aggregate to **9** distinct `(pre_type, post_type)` pairs, and the
manifest's `edge_count` is `cols.len()` — the CSR nonzeros — so the builder prints `9 edges`. The
plan's prose conflates source segment rows with emitted type-level edges; the CSR is the artifact, so
the CSR count is the number reported.

## D-006 — Kernel sources live in `kernels/`

The plan puts new kernels under `crates/cuda/kernels/`. The engine's existing convention is a
workspace-root `kernels/` directory embedded with `include_str!`. The root `kernels/` is kept as the
single location so there is one convention rather than two.

## D-007 — The mark lives in this repository

The plan says the psi geometry is authored "in the journal repo". `superposition/qualia` owns
`assets/mark/{psi.json,render_mark.py,psi.svg}` so the flat mark and the 3D mesh cannot drift from
each other or from a second copy; the two sites receive the inlined geometry and the favicon in
ticket T41.

## D-008 — Runner operator logs are interface

Runner log lines (`qualia-pose: pose_seq=…`, `qualia-lidar: scan ok`, …) are operator-visible output:
the contract pass checks them line for line against the reference, and operators and the journal read
them. They stay byte-identical to the reference even where a label (`tx`, `weight`, `keyframes`) uses
vocabulary the re-authored internals no longer share.

`scripts/provenance_check.py` is the arbiter of copied text — it measures code runs and prose runs,
and a single-line format string is below its thresholds. Ticket #91 does not list the log lines among
the things its rewrite changes, and the lidar scrub (#137) treated the same question the same way.
C25's clean-room request to re-author the two `qualia-pose:` format strings is declined on this basis;
the composition-order finding in #135 stands.

## D-009 — Emitted values are interface

A rewrite keeps every value the binary emits, publishes or writes observable — log text (D-008), the
defaults it seeds (world directives, offline scene and object names), the codes and kinds it publishes
(thought kinds, exposure labels), stderr diagnostics, and the fields it leaves unset — identical to the
reference. Re-authoring concerns internals: names, structure, control flow, comments. A rewrite that
re-words an emitted value, drops a diagnostic, or starts writing a field the reference leaves zero is a
contract change, and the contract pass requests it back.

Established by the C35 vision (#144) and C34 camera (#151) reviews on 2026-09-11: both had re-authored
values with no consumer-visible justification, and the tickets name only crate internals as the subject
of the rewrite.

## D-010 — The deploy target "Pinkie" is attached to the dev host

Pinkie is the Jetson Orin NX Engineering Reference Developer Kit (aarch64, L4T R36.4.7, CUDA 12.9,
6 cores, ~3.6 GiB RAM visible, 161 GiB free on `/`) that carries the robot's software: Leash runs there
(`leash` listening on :8000) with llama.cpp serving on :8080. It is the machine the plan calls "the
Orin Nano" and the place the stack is actually used.

Access from the dev host: Pinkie presents as a USB composite device — `UsbNcm Host Device` at
`192.168.55.100/24` with the board at `192.168.55.1`, plus a serial console on `COM3` (both NVIDIA
gadget interfaces, VID 0955). `ssh -i ~/.ssh/qualia_jetson_ed25519 jetson@192.168.55.1` works; the
account password is `jetson`.

Development stays on the 4090 host; deployment is a cross-build (`docker/Dockerfile.cross-aarch64`,
`Cross.toml`) or a native aarch64 build on the board itself. Kernels are developed and verified on the
4090 (sm_89) and ship for sm_87 (`Makefile CUDAARCHS`). Three board facts measured by the T29 smoke
(issue #45): the board has **no DNS** (ship a tarball; `git clone`/crates.io are unreachable), its clock
is ~10 days behind the dev host (TLS to anything remote will complain), and a Windows-side clone writes
CRLF into the working tree, which breaks shell scripts and the cross-build Dockerfile on Linux — ship
`git archive` output, not a working copy.

## D-011 — GPU work is serial because the host's stability risk is the display driver

The 2026-09-11 session that took the swarm down was followed by Windows System events
`nvlddmkm` Id 153 (display-driver error) at 05:53 — while the machine's SSD (`WD_BLACK SN850X`,
`HealthStatus = Healthy`) and RAM (never above ~21 GiB of 64) showed nothing wrong. Two storage
signals appeared the same day: the WSL2 rootfs aborted its ext4 journal into a read-only mount, and
the deploy clone wrote CRLF.

Consequences for every agent:

- **One GPU-touching command at a time**, never two CUDA jobs concurrently, and prefer bounded runs
  (a test binary with a timeout, a capture with an iteration cap) over open-ended loops or benchmarks
  that can occupy the device for minutes.
- `ncu`/`nsys` captures count as GPU work; announce them (as `ProfileJepa` did) so peers hold off.
- WSL2 output directories go on the Windows filesystem (`/mnt/c/...`) when the ext4 mount has logged
  errors; the committed evidence stays small and in-repo per `docs/evidence/README.md`.
- A read-only WSL mount is recovered with `wsl.exe --terminate <distro>` (journal replay); do not fsck
  a mounted root and do not rebuild the distro while it boots.

## D-012 — The 06:10 crash was a script disabling the display adapter; the profiling target is Pinkie

`C:/tmp/nvprof-fix.ps1`, run at 06:10:41 by the T50 profiling work in an attempt to make
`RmProfilingAdminOnly` take effect, ran `Get-PnpDevice -Class Display | Disable-PnpDevice`, slept four
seconds and re-enabled. Its log stops at `display-disabled`; two seconds later the harness's stdout
broke (`EPIPE`, 06:10:43), every agent session was disposed, and both adapters (`NVIDIA GeForce RTX
4090`, `Meta Virtual Monitor`) sat at `CM_PROB_DISABLED` (Code 22) — CUDA and `nvidia-smi` with them —
until an elevated `Enable-PnpDevice` at 06:16. This is a separate event from the `nvlddmkm` Id 153 TDR
D-011 records at 05:53; both are display events, and neither was concurrent GPU work.

Consequences:

- No script, agent or test disables, enables or restarts a display adapter, or reinstalls its driver.
  A device toggle is a host outage, not a step. A tool that says it needs one is a stop-and-ask.
- `ERR_NVGPUCTRPERM` on the dev host is not a project requirement. The profiling target is Pinkie —
  the Waveshare-carried Jetson Orin NX at `jetson@192.168.55.1` (L4T R36.4.7, sm_87, 6 cores,
  3.6 GiB RAM, 161 GiB free, account password `jetson`, `ncu` at `/usr/local/cuda/bin/ncu`, no
  `nsys`, no `nvcc`). Profiling evidence is captured there and ships with the ticket. The host's
  `RmProfilingAdminOnly=0` was set by the same attempt; it is left as-is, is not required by anything,
  and must never be forced to take effect by a device restart.
- D-011's serial-GPU rule stands (one GPU job at a time, bounded runs). Its attribution is amended:
  the session it followed was killed by the scripted device disable above, not by GPU work.

## D-013 — The host guard, and the build budget it enforces

`C:/tmp/resmon2.py` runs under the harness process supervisor (`hub ps`, name `resmon2`, `persist: true`)
and samples RAM, pagefile, VRAM and build-process count every 10 s into `C:/tmp/resmon.log`. Levels:
`PRESSURE` under 8 GiB available, `HIGH` under 4 GiB (kills the two largest rustc/cl/link processes),
`CRITICAL` under 2 GiB (kills the four largest build processes). Killing a build costs a rebuild;
letting the host run out of memory costs the session.

The budget this enforces, stated for every agent in [`agents.md`](agents.md) §Host safety: agent builds
use `cargo -j 2` (D-014 tightens this from the original `-j 4` while the CPU fault lasts); no
workspace-wide build/test matrices; GPU work is one bounded job at a time; profiling
happens on Pinkie. Measured 2026-09-11 with 21 agents building at once: 19.5–21.7 GiB of 63.9 GiB used
and pagefile slack ≥ 41 GiB, so the guard is a ceiling, not a routine actor.

## D-014 — The host CPU is throwing corrected machine checks; builds are throttled and re-verified

Since 05:44 local on 2026-09-11 the host logs `Microsoft-Windows-WHEA-Logger` Id 19 events: *"A corrected
hardware error has occurred. Reported by component: Processor Core / Error Source: Corrected Machine
Check / Error Type: Internal parity error."* Eight events by 06:27 local (05:44:09 ×2, 05:44:38,
06:20:36 ×2, 06:21:58, 06:23:08, 06:27:43), i.e. **one every one to three minutes while the swarm
builds** — not a one-off.

The user-visible symptom: rustc dies nondeterministically with const-eval ICEs whose reported values are
garbage — `scalar size mismatch: expected <different garbage each run> bytes but got 8 bytes`,
`primitive read not possible for type: usize`, `the compiler unexpectedly panicked` in
`eval_to_allocation_raw` — on unrelated crates (`windows-sys`, `zmij`, `paste`, `ring`, `owo-colors`,
`icu_locale_core`) while the same command line sometimes succeeds. Reported by six agents; a fresh
minimal crate using the same windows-sys version compiles, and the same source at the same commit
alternates between success and ICE, so the variable is the host, not the code.

Consequences, in force until the operator clears the fault:

- **Builds are throttled and don't count as evidence until re-verified.** `cargo -j 2` at most, one build
  at a time per agent, no workspace-wide builds. A result obtained while a WHEA event was within ±2
  minutes is provisional; in practice the valid bar is two agreeing runs spanning an event. Keep the exit
  code, not a piped `tail`.
- **An ICE is a host fault, not a crate bug.** Retry once at `-j 1`; if it recurs, post
  `blocked_on: host CPU fault (WHEA 19)` and stop. Do not hunt a compiler or dependency defect.
- **Mitigation applied.** Processor maximum state is 99% on AC and DC (`PROCTHROTTLEMAX`), which
  disables turbo boost and its voltage excursions; the previous value was 100%. Revert with
  `powercfg /setacvalueindex SCHEME_CURRENT SUB_PROCESSOR PROCTHROTTLEMAX 100` (and `/setdcvalueindex`,
  then `/setactive SCHEME_CURRENT`). `C:/tmp/resmon3.py` samples RAM/VRAM/build count, reads WHEA events
  every minute into `C:/tmp/resmon.log`, and stamps `C:/tmp/host_fault_window.txt` on each new event.
- **Operator action is required for a real fix**: update the BIOS/microcode for this Raptor Lake
  i9-14900KF and select the Intel-default (not unlimited) power profile; if events persist, the CPU
  needs an RMA. Software throttling reduces but does not remove the corruption.

## D-015 — The 07:01 local crash, the five-agent cap, and the restarted guard

The host went down without a clean shutdown at ~07:01 local (11:01Z) on 2026-09-11 while the swarm ran
about 21 agent sessions; the `Kernel-Power` Id 41/6008 pair is the third such pair that day (boots at
01:53, 05:23 and 07:05 local), and the last WHEA Id 19 before it is 06:27:43 local. The swarm's GitHub
writes stop at 11:01:31Z, so the crash — not an agent bug — is what killed the 10:11 session;
`docs/agents.md` §Recovery applies (read GitHub, never a dead session's memory).

Operator rule on restart (12:05 local): **at most five agent jobs run at once.** The 10:18 batch ran 21
(`C:/Users/ericm/.omp/agent/sessions/-qualia/2026-09-11T10-11-06-266Z_01a08ff3-0f9a-735c-8182-5768b6afb43a.jsonl`);
the cap is the operator's condition for restarting the tickets, not a memory measurement (D-013's
numbers stand).

The guard's supervised name is `resmon5` from this restart: the `resmon4` record in the supervisor was
left completed-but-unstartable after the crash. Same script (`C:/tmp/resmon4.py`), same log
(`C:/tmp/resmon.log`), `persist: true`; read `hub ps` / `hub logs` under the new name.

## D-016 — Board build facts the legs discovered (fp16, the crate cache, the overlay)

Measured by the restart's first board job on 2026-09-11 (its comments on PRs #169/#165/#180/#184/#192,
issues #75/#76/#108/#38/#102); every future board build inherits these:

- **candle-bearing crates need `RUSTFLAGS="-C target-feature=+fp16"` on the board.** `gemm-f16`'s
  inline asm is rejected by the default `neon`-only aarch64 target ("instruction requires: fullfp16");
  the Orin's A78AE has the feature. Invisible on x86_64, absent from `Cross.toml`, and not needed by
  non-candle packages.
- **The board's crate cache is not the dev host's.** It had no `turso` at all; the 0.7.2 closure
  (103 `.crate` files plus index entries) was shipped to `~/.cargo` by hand. Offline board builds of
  any new dependency set must ship its closure the same way.
- **Merge order matters for board trees.** C10/C42 could not build at their own heads (they call the
  C09 crate); their legs were cut as head + `crates/jepa-model` at `7b30678f`. After `ea823dd`
  (#169), `ad30020` (#165) and `d7ab633` (#180) landed, that overlay is no longer needed.
- **A board run is one job at a time.** Board legs are serialized through one agent; the queue is
  Main's, not a free-for-all.

## D-017 — What an `ncu` capture's `metrics.csv` actually is, and mage's parser

Measured on 2026-09-11 by the board's T50 leg (its comment on PR #183; artifacts `C:/tmp/T50raw/`),
live on Pinkie with ncu 2025.3.1:

- `ncu --csv --page raw` writes a 291-column wide table (metric identifiers as headers, one row per
  launch; a units row under the header). mage's `NcuBackend._parse_csv_output` rejects it:
  `Unsupported Nsight Compute CSV schema`. mage's parser requires the long form (`Metric Name`,
  `Metric Value`, `Metric Unit`, `Kernel Name`) — i.e. `--csv`'s default **details** page, which is
  what the committed T50 exports are.
- mage's backend asks ncu for `--page raw` while its parser expects details: that is a mage defect,
  outside this repository (recorded here so no ticket tries to satisfy both readings at once).
- A details export that names `launch__shared_mem_per_block_static` carries unit `byte/block`, which
  mage's `_parse_value` rejects (its byte scale accepts only byte/kb/mb/gb). A capture whose
  `--metrics` includes that counter will not parse in mage; the directory README must say so.
- The same two runs write `capture.ncu-rep` reports of ~594 KB each — commit them when the tree's
  size budget allows, and name them in the README when not (the T50 reports are committed).

`docs/evidence/README.md` now names the details page as `metrics.csv`; the T50 capture stands with
its `.ncu-rep` reports attached.

## D-018 — The dev host cannot cross-build for the board; board builds are native (amended below)

Measured 2026-09-11 by both board jobs: the Docker engine is unreachable, `cross` is not installed,
and a direct `cargo build --target aarch64-unknown-linux-gnu` dies in `ring`'s build script for want
of `aarch64-linux-gnu-gcc`; the musl target fails the same way (`aarch64-linux-musl-gcc`). The
"build here, run there" lane D-010 names for C11 is therefore unavailable on this host today. The
lanes that work: **(a)** a native aarch64 build on Pinkie from a `git archive` of the head (the T50
capture and the C39/T23/T26 legs use it; the board's crate cache is populated), and **(b)** shipping
a host-built artifact only when it is architecture-independent. Operator action to restore the cross
lane: install the `aarch64-unknown-linux-gnu` toolchain, start Docker, or install `cross`.

**Amendment, later on 2026-09-11 — the cross lane is restored, and restoring it was an agent action,
not an operator one.** The paragraph above measures the host *as it was installed at that moment*,
not the host. Taking its three operator remedies in turn, on the same machine:

- **"install the `aarch64-unknown-linux-gnu` toolchain" — needed.** It is the first line of the recipe
  below; without the target's std nothing cross-compiles.
- **"start Docker" — not needed.** D-010's `Cross.toml` image stays a route, but it wants a running
  engine, and building that image wants network and root.
- **"install `cross`" — not needed.** Its job, driving a cross toolchain over a
  `aarch64-linux-gnu-gcc`, is done here by `cargo-zigbuild`, which needs no root.
- **the Debian `aarch64-linux-gnu-gcc` that `ring` failed for — not needed either.** Zig *is* that
  compiler, plus a glibc sysroot, and `uv` installs it into `$HOME` from a Python wheel.

`sudo` needs a password on this machine, so nothing in the lane may ask for root; nothing does.

Replayable recipe, on the WSL2 Ubuntu-22.04 guest. Two prerequisites are not in the tree: `rustup`,
and `uv`, which the third line needs and which rustup does not supply — the guest carries `uv 0.6.11`,
and upstream's rootless installer is `curl -LsSf https://astral.sh/uv/install.sh | sh` (the project's
documented route; not measured here, because this guest already had `uv`):

```bash
rustup toolchain install 1.94.1 --profile minimal --target aarch64-unknown-linux-gnu
rustup toolchain install 1.98.1 --profile minimal          # cargo-zigbuild's rust-version is 1.88
uv tool install ziglang==0.16.0                            # the wheel ships the compiler; no root
ln -sf "$HOME/.local/share/uv/tools/ziglang/bin/python-zig" "$HOME/.local/bin/zig"
cargo +1.98.1 install cargo-zigbuild --version 0.23.4 --locked -j 2

git -C <checkout> archive HEAD | tar -x -C <a fresh tree>   # build an export, not the checkout
cd <that tree>
export CARGO_TARGET_DIR="$HOME/scratch/target-cross"
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS="-C target-feature=+fp16"
cargo +1.94.1 zigbuild --release --target aarch64-unknown-linux-gnu.2.17 \
  -p qualia-jepa-model --bins -j 2
```

Two properties to know before running the recipe:

- **Its first run needs the network.** All three installs above download; there is no offline form of
  the first run.
- **It mutates shared per-user state on the guest, not the repository:** `~/.rustup` (two toolchains
  and the aarch64 std), `~/.cargo/bin/cargo-zigbuild`, and `~/.local/bin/zig`. Re-running it is a
  no-op, but it is not a sandboxed build step, and it is not a per-worktree one.

Three details worth naming:

- **`.2.17` is the glibc floor, and it is the right one.** aarch64 Linux starts at glibc 2.17, Rust's
  prebuilt std for this triple wants 2.17, and the board is Ubuntu 22.04.5 / `GLIBC 2.35` (D-010), so
  the artifacts run there. Naming a *higher* floor narrows where the artifacts run and buys nothing.
- **The `+fp16` flag is required, and a target-scoped variable is where it is set.** `gemm-f16`
  0.18.2 is in `qualia-jepa-model`'s graph and compiles for the target, and D-016's flag is what lets
  its inline asm assemble there. `CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS` states by
  construction that the flag belongs to the target crates only. Measured on a two-crate probe (a
  proc-macro and a build script) cross-`check`ed for this triple: plain `RUSTFLAGS` also reached one
  unit only — the target crate, and not the host build script or the proc macro — and exited 0, and
  the target-scoped variable behaved identically. It is a statement of intent, not a workaround, and
  not something that failed here.
- **A `zig` command has to exist on `PATH`.** `uv tool install ziglang` installs only a `python-zig`
  entry point; the symlink is what `cargo-zigbuild` looks for.

Measured with: rustup 1.29.1, rustc 1.94.1 (e408947bf 2026-03-25) building the target, zig 0.16.0
(the `ziglang` 0.16.0 wheel), cargo-zigbuild 0.23.4 built by rustc 1.98.1 (48a229cea 2026-09-01).
The four binaries build in ~2m40s at `-j 2`, and a replay from a fresh `git archive` into a fresh
`CARGO_TARGET_DIR` produced byte-identical files — the same SHA-256 for all four. **That
byte-identity is version-tied**: it is a property of these pinned versions, not of the recipe, so a
reader who moves any pin should re-measure it rather than assume it.

At `60beee4`, `cargo zigbuild --release --target aarch64-unknown-linux-gnu.2.17 -p qualia-jepa-model
--bins` yields four artifacts that `file` reports as

```text
qualia-jepa-train:         ELF 64-bit LSB pie executable, ARM aarch64, version 1 (SYSV), dynamically linked, interpreter /lib/ld-linux-aarch64.so.1, for GNU/Linux 2.0.0, stripped
qualia-jepa-parity:        ELF 64-bit LSB pie executable, ARM aarch64, version 1 (SYSV), dynamically linked, interpreter /lib/ld-linux-aarch64.so.1, for GNU/Linux 2.0.0, stripped
qualia-jepa-plan-eval:     ELF 64-bit LSB pie executable, ARM aarch64, version 1 (SYSV), dynamically linked, interpreter /lib/ld-linux-aarch64.so.1, for GNU/Linux 2.0.0, stripped
qualia-jepa-runtime-probe: ELF 64-bit LSB pie executable, ARM aarch64, version 1 (SYSV), dynamically linked, interpreter /lib/ld-linux-aarch64.so.1, for GNU/Linux 2.0.0, stripped
```

Their only `NEEDED` entries are `libm.so.6`, `libc.so.6`, `libpthread.so.0` and `libdl.so.2`, and the
highest glibc symbol version any of them imports is `GLIBC_2.17`. The board run is recorded in
[`evidence/board/cross-lane/README.md`](evidence/board/cross-lane/README.md).

Consequences:

- **D-018's two routes keep their letters and gain a third.** **(a)** a native aarch64 build on
  Pinkie, unchanged. **(b)** shipping a host-built artifact when it is architecture-independent,
  unchanged. **(c) — this amendment — a host cross-build with `cargo zigbuild`**, for artifacts that
  are architecture-dependent Rust whose dependency graph this host can satisfy. D-010's `Cross.toml`
  image remains a further route whenever a Docker engine is running. (c) needs no root, no Docker and
  no board slot.
- **Which route, when.** Cross-build (c) when the change is Rust the crate graph covers and the
  artifact must run on the board: it is the only route that costs no board time, and it leaves the
  board's crate cache untouched. Build natively on the board (a) when the build needs something this
  host cannot install rootlessly — today, the CUDA feature's aarch64 toolkit — or when the board's
  own toolchain is what is under test. Ship host-built bytes alone (b) only when they are
  architecture-independent.
- **Scope of the lane.** It builds the board's own target, `aarch64-unknown-linux-gnu`, which is the
  architecture DoD's "built for the robot's architecture" asks for. It does not touch the host-only
  case DoD names — `--backend metal` (`crates/jepa-model`'s `metal` feature) still records an aarch64
  build as its board evidence, with execution on the board impossible — and it does not build
  `--features cuda`.
- **Native board builds stay valid.** The board lane is not deprecated by this; (c) removes the
  host's *dependence* on it, which is what D-018's *lanes that work* list was really recording.
- **The cross lane does not build the CUDA feature, and that is a toolkit gap rather than a cross
  gap.** `--features cuda` for `aarch64-unknown-linux-gnu` needs the aarch64 CUDA toolkit (`nvcc`,
  `libcudart`) that this host does not carry, so a default-feature cross build refuses a CUDA target
  with `backend cuda is not compiled into this binary`. The cross lane therefore proves that the
  board *runs* the model binaries, not that it runs them on the GPU.
- **The C path is untested by this package.** `qualia-jepa-model`'s graph is Rust-only — `cargo tree`
  names no `cc` and no `ring` — so this build never exercised D-018's `ring` failure. cargo-zigbuild
  does export `CC`/`AR`/`CXX` wrappers around `zig cc` for the target, which is the mechanism that
  should answer it; that is `[INFERENCE]`, not a measurement.

## D-019 — A PR based on another ticket's branch can strand its work; base PRs on `main`

Instance: PR #185 (T12, ticket #27) was opened with base `ticket/T11` and merged into that branch at
`f2ad305` on 2026-09-11 10:52Z, which closed #27. When `ticket/T11` was later rebased onto `main`,
the rebase dropped the merge commit and with it T12's commits (`53233ce` and siblings), so the work
existed only in the merge commit's parents — an ancestor of neither `main` nor the branch. The
stranded work is being replayed (`ticket/T12-replay`, `Closes #27`).

Consequences:

- **PRs are based on `main`.** A dependency that has not landed yet is stated in the ticket body and
  recorded in a `blocked_on:` field, never expressed as another branch as the PR base.
- **Closing is not landing.** A `Closes #n` in a PR that merges into a non-default branch closes the
  issue without the work reaching the trunk; the tracker lies until someone checks. When a ticket's
  work is found stranded, replay it and reopen the issue rather than trusting the label.

## D-020 — The kernels' value contract is the accumulation order and the `fmaf` pin

The CPU reference in `crates/cuda/src/cpu.rs` is the oracle the kernels are checked against, and its
module doc (`cpu.rs:14-22`) makes the *bits* part of the twin, not only the order of operations.
Instance: #221 vectorized both row walks and the emitted bits moved — 2.9 % of the weights one ULP
apart — until the scalar multiply-add's contraction was pinned with `fmaf` and the accumulation kept
in ascending column order.

- **The order is interface.** Every walk accumulates its row in ascending column order; a tiled or
  shared-memory reduction re-associates the sum, so #172's step-2 recommendation (a tiled reduction)
  was refused in #221 rather than deferred. The refusal is recorded above each vector loop in
  `kernels/belief_update.cu` and `kernels/cognition_update.cu`.
- **The contraction is interface.** nvcc chooses FMA contraction per instruction, so the `fmaf` pin is
  what makes the result reproducible; without it, a future vectorization moves a few percent of the
  weights by one ULP and every test stays green.
- **No test catches either.** The device comparison is `1e-5 + 1e-4 × scale`
  (`crates/cuda/tests/gpu.rs:43-51`), which a one-ULP move passes; D-009 makes the emitted values
  interface, so the contract lives in the doc and the loop comments, and the enforceable form is an
  FNV-1a probe over the `gpu.rs` fixtures.

## D-021 — The rank gate reads a representation training never moves; T52's fix is that measurement

Ticket [#225](https://github.com/superposition/qualia/issues/225) (T52) asked for the checkpoint the
promoted end-to-end run needs, and for a choice among three ways to get one: more/different training
data, a calibrated gate, or a smaller model. Six bounded configurations on the 4090 — seven
processes, since one is an instrumented re-run of another — (`docs/evidence/T52/checkpoint-experiments/`)
answer it, and the answer is not data and not capacity.

**The measured basis.**

- The gate-minimum static fixture still cannot publish: on CUDA at one epoch the trainer refuses
  with `validation.flat_mlp.rollout_error = NaN`. Its rank is `1.37182`, its calibration slope
  `3.254` against the required `[0.9, 1.1]`, and `clamp_fraction 0.710` against `≤ 0.01`.
- A non-degenerate fixture (a moving, action-conditioned grating; `wave-fixture.rs`) publishes, so
  the `NaN` is a property of a degenerate fixture, not of the trainer. Holding the recipe and
  lengthening only the *chain break* moves the rollout gate: `6.09e8` at chain 64, `3.743` at chain
  16, and **`0.129` against the constant baseline's `1.485`** at chain 4 — where the predictive gate
  and occupancy pass outright and `clamp_fraction` is `7.8e-5`.
- Everything except the rank is reachable: `small-e1` (the predictor's hidden width 512 → 64 — a
  temporary change, reverted before the commit; the code at HEAD is `PREDICTOR_HIDDEN_DIM = 512`,
  and the run's manifest still declares `architecture_id: qualia.jepa.grounded-tiny-cnn.v1`) puts
  the whole test split inside the calibration bands (`slope 0.973`, `mean_standardized_squared_residual 0.924`,
  coverage `.519/.920/.957`, `clamp 3.1e-4`) on top of the predictive and occupancy gates.
- `effective_rank` never moves. Four fixtures whose inputs differ by orders of magnitude read
  `1.372 / 1.084 / 1.120 / 1.120` against a floor of `64`; and on the same fixture and seed the
  representation is **byte-identical at one and ten epochs** — `encoder-freeze-diff.txt` shows the
  predictor's tensors moving by up to `2.62` while all 20 `encoder.*` and all 20 `target_encoder.*`
  tensors differ by exactly `0.000000e+00`.
- Why: the encoder is not in the training objective's gradient. `gradient-probe.txt` reports
  `predictor.output.weight` at `2.97e2` and **every `encoder.*` parameter absent from the backward
  pass** (22 of the map's 30 variables, `is_variable() == true` among them); `visible.sum_all().backward()`
  yields zero leaf variables, while mutating the map's `encoder.camera.conv1.weight` does change
  `visible`, so the forward reads the map's storage but builds no gradient leaves for it. The EMA
  target is frozen with it, and `effective_rank_report` is computed over that EMA target
  (`crates/jepa-model/src/bin/qualia-jepa-train.rs`, `measure_holdout`).

**The decision.** The fix is the second option — **a calibrated gate** — with the emphasis on the
measurement rather than the threshold: the rank gate is fed by a held-out representation the
pipeline never trains, so its verdict is not a function of the training the pipeline performs.
Lowering `64` to anything above `1.37` would fit a band to a broken number. No threshold moves in
this decision; the repair it names is a code change, and it lives in **T53
([#228](https://github.com/superposition/qualia/issues/228))**, opened from this capture with this
measurement and the test that must fail today. More data is ruled out by measurement (the rank is
invariant across four fixtures and across a ten-fold epoch sweep), and a smaller model is ruled out
as the *fix* while being real for calibration (it moves the calibration bands, not the rank). The
repair owed is the encoder/EMA wiring the probe measured — the online encoder must be in the
objective's gradient — after which the band is re-derived and the sweep re-run.

**What this does not establish.** It does not establish that a repaired encoder clears the rank
floor, and it does not establish that it then clears calibration: `ch4-e10` shows the calibration
*growing worse* with more training (`slope 1.430 → 0.426`, `clamp_fraction 7.8e-5 → 0.595` over
1 → 10 epochs), so a repaired encoder may still fail the calibration bands and the sweep has to be
re-run on the repaired pipeline rather than assumed to come out green. It also does not isolate the
mechanism — the probe measures that no gradient leaf exists for the encoder, not which detach site
produces that; that gap is #228's test.

**Consequence.** No artifact this repository can publish clears the promotion gates today, so T52's
*produce* half — "produce (or obtain) a checkpoint whose held-out gates pass" — and with it step 2,
`qualia-jepa-parity` and `qualia-jepa-plan-eval` against a **promotion-passing** checkpoint, **remain
blocked on that repair**; they are not blocked on a fixture shape and not on training time, and the
decision half of step 1 is discharged by this entry. The parity and plan-eval step times themselves
are already measured against a loadable candidate (`docs/evidence/T50/model-eval/`); what is missing
is only the promoted checkpoint's provenance. Step 4's board limb is recorded on its stated reason
in the capture: no aarch64 artifact can be built here (#224's `rc 101`, `error[E0463]`, D-018) and
none can run on Pinkie (no `candle-core` in the offline cache, no DNS; D-016).

## D-003 — Repository


Public repository is `superposition/qualia`. The former private repository is
`superposition/qualia-private` (archived). The plan's `specdog/qualia` owner is **not** used:
`specdog` is a third-party account even though the operator has access to it. Leash
(`specdog/leash`), the MIT project cited in `NOTICE`, is the operator's own project.
