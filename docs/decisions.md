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
in CI by `.github/workflows/notice-check.yml`.

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

## D-003 — Repository

Public repository is `superposition/qualia`. The former private repository is
`superposition/qualia-private` (archived). The plan's `specdog/qualia` owner is **not** used:
`specdog` is a third party.
