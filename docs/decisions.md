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

## D-003 — Repository

Public repository is `superposition/qualia`. The former private repository is
`superposition/qualia-private` (archived). The plan's `specdog/qualia` owner is **not** used:
`specdog` is a third party.
