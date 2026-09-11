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
use `cargo -j 4`; no workspace-wide build/test matrices; GPU work is one bounded job at a time; profiling
happens on Pinkie. Measured 2026-09-11 with 21 agents building at once: 19.5–21.7 GiB of 63.9 GiB used
and pagefile slack ≥ 41 GiB, so the guard is a ceiling, not a routine actor.

## D-003 — Repository


Public repository is `superposition/qualia`. The former private repository is
`superposition/qualia-private` (archived). The plan's `specdog/qualia` owner is **not** used:
`specdog` is a third-party account even though the operator has access to it. Leash
(`specdog/leash`), the MIT project cited in `NOTICE`, is the operator's own project.
