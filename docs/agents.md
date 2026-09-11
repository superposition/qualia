# Agents: claims, breadcrumbs, handoffs

The state of the work lives in GitHub, not in an agent's memory. An epic is an issue, a ticket is an
issue, a claim is an assignee plus a comment, and a handoff is a comment in a fixed format that any
later agent can parse.

## The breadcrumb

Every claim, handoff and blockage is one comment whose first line is a fenced block:

````text
```braid
agent: <name>
branch: <branch>
commit: <sha or none>
state: claimed|working|blocked|review|done
next: <the single next action, imperative>
blocked_on: <ticket or external thing, or none>
evidence: <path or URL, or none>
```
````

The remainder of the comment is prose for a human.

A working agent posts a fresh block at every step boundary — when a step starts and when it ends — so
the comment stream, not the agent's memory, is the state. Keep the fields true at the moment you post:
`commit:` is the last pushed commit, `next:` is one imperative action, `evidence:` is a path or URL a
later agent can open. Posting the block before a long step is what makes a crash cost one step instead
of a session.

## Claiming

A claim comment also runs:

```bash
gh issue edit <n> --repo superposition/qualia \
  --add-assignee @me --add-label status:claimed --remove-label status:ready
```

## Handoff

Finishing work is a pull request whose description begins with the same `braid` block and ends with
`Closes #<n>`. If the PR is unmerged at handoff, the comment says so in `next:` and the issue keeps
`status:review`.

## Review

Nothing reaches `main` without three reviewers, three different jobs. Each posts one comment on the
PR, its first line a fenced block:

```text
role: correctness|clean-room|contract
verdict: approve|request-changes
notes: <count>
```

- **correctness** — the diff does what the ticket's steps say; error paths and boundaries covered;
  the tests assert observable behaviour rather than wiring; the package's build and tests pass, and
  the evidence in the PR is evidence you reproduced yourself.
- **clean-room** — `python scripts/provenance_check.py` prints `OK`; no comment or doc prose is shared
  with the reference, and any long identical code run is a declaration the contract forces (constant
  table, enum variant, field list, manifest key), not copied logic.
- **contract** — package name, feature flags, public types, function signatures, constants and wire or
  JSON field names match the reference interface; the diff touches only the files the ticket names.

A `request-changes` verdict is answered by a fix and a re-read from that same reviewer; the other two
verdicts stand unless the fix touched their concern. The merge happens only when all three read
`approve`, and the merge commit's message names the ticket.

## Blockage

A blocked ticket gets `status:blocked`, `blocked_on:` filled, and a `resume` label only when the
blocker is another agent that may never return.

## Resume

```bash
gh issue list --repo superposition/qualia --label ticket --state open \
  --json number,title,labels,assignees,updatedAt --limit 100 \
  | jq -r '.[] | select([.labels[].name] | index("status:ready")) | "\(.number)\t\(.title)"'
```

For anything already claimed, the last `braid` block in the issue's comments is the handoff: read it,
and if `next:` is actionable, continue; if its `state:` is `claimed` and `updatedAt` is older than four
hours, relabel it `resume` and take it over with a comment explaining that you did. Never edit another
agent's breadcrumb — add a new comment.

## Recovery after a crash

Recovery reads GitHub, never a dead session's memory.

1. `gh issue list --label ticket --state open` and take the ticket whose last block is stale.
2. Read the issue's last `braid` block. `git fetch` the branch it names and compare `commit:` with
   `git log` in its worktree; work past that commit is the step that was in flight.
3. Run the ticket's own commands and `python scripts/provenance_check.py`; keep what passes, redo the
   rest.
4. Post your own block with `state: working` and the recovered `commit:`, then continue from `next:`.

If a worktree file is unreadable or filled with NUL bytes, the machine died mid-write: restore it with
`git checkout -- <path>` — the content was either committed or lost with the step in flight — and say
so in the block.

## Host safety

The dev host is shared by every agent and cannot be rebooted cheaply. Four rules, from
[`decisions.md`](decisions.md) D-011/D-012/D-013:

- **Never touch display devices.** No `Disable-PnpDevice`/`Enable-PnpDevice`/`pnputil` on a display
  adapter, no driver reinstall, no profiler-permission script. The 06:10 crash was exactly this (D-012).
- **One GPU job at a time, bounded.** `--test-threads=1`, iteration caps, no open-ended benchmarks;
  `ncu`/`nsys` runs count. Profiling happens on Pinkie (the board), not the host.
- **Build bounded.** `cargo ... -j 2` for agent builds while the CPU fault lasts (D-014; the general
  budget is D-013); no `--workspace` build/test matrices, no
  unbounded test loops; prefer one package at a time.
- **The CPU itself is currently faulty.** Since 05:44 on 2026-09-11 the host logs WHEA-Logger Id 19
  corrected machine checks (processor core, internal parity error) every 1–3 minutes under load; rustc
  intermittently dies with garbage-value const-eval ICEs as a result. Build with `cargo -j 2` at most,
  one build at a time, never a workspace build; keep the exit code, not a piped `tail`. An ICE is a host
  fault: retry once at `-j 1`, then post `blocked_on: host CPU fault (WHEA 19)` and stop. A result
  obtained while a WHEA event landed within ±2 minutes is provisional — re-run it, or show two agreeing
  runs spanning an event. Static work (git, Python, the gate, diff reads) is unaffected. See
  [`decisions.md`](decisions.md) D-014.
- **Watch the guard.** `C:/tmp/resmon4.py` runs persistently (`hub ps`, name `resmon4`) and logs to
  `C:/tmp/resmon.log`: RAM/VRAM/build count every 10 s, new WHEA events as `WHEA …`, and the last fault
  stamped at `C:/tmp/host_fault_window.txt`. Under memory pressure it kills the largest build processes
  rather than let the box OOM. If you see `WARN[HIGH]`/`WARN[CRITICAL]`, reduce your footprint and say so
  in your braid.

## Definition of done

A ticket is done when its package builds and its tests pass on the dev host, its artifact is built for
the robot's architecture and exercised on **Pinkie** — the Waveshare-carried Jetson Orin NX at
`jetson@192.168.55.1` (see [`decisions.md`](decisions.md) D-010) — by the ticket's own smoke path, and
the handoff comment quotes that build/deploy command and the output observed on the board. A host-only
package that cannot run there (the macOS `metal` backend) records the aarch64 build as its board
evidence and states why execution on the board is impossible.

The board is a deployment target, not a build farm: build here, copy the artifact there, run it there,
quote what it printed.
