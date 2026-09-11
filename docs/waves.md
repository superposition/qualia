# Waves

Parallelism is by ticket, never by file. Within a wave, each ticket owns the files its `## Steps`
names. Two tickets that name the same file are not in the same wave. `runners/agent/src/main.rs` is
owned by one ticket per wave.

| Wave | Tickets | Why together |
| --- | --- | --- |
| 0 | C01–C43 (engine rewrite) | Dependency-ordered within the wave; see the crate order below. |
| 1 | T04–T09 | Fixtures and the prior crate; one directory, one owner. |
| 2 | T10–T18 | Belief coupling, kernels, parity. |
| 3 | T19–T22, T30–T32 | Braid, missions, self-improvement. |
| 4 | T23–T27, T33–T36 | Exploration, front end, seam. |
| 5 | T37–T43 | Journal and mark. |
| 6 | T28–T29 | End-to-end runs, alone at the end. |

A wave ends when every ticket in it is `status:done` or `status:blocked` with a filled `blocked_on:`,
**and** each epic the wave completed has a live journal entry:

```bash
gh issue list --repo superposition/qualia --label epic --state open --json number,title
```

must not list an epic whose tickets are all closed.

## Crate order inside wave 0

Interfaces are fixed by the private engine's public API; implementations are written here.

```
types
  -> shm, ipc, sync-types
       -> mcap-log, session-store, jepa
            -> jepa-dataset, jepa-model, jepa-registry, rerun-bridge
            -> cuda, metal
                 -> runners/*, studio/rust, apps/qualia-console
```

## Resume

```bash
gh issue list --repo superposition/qualia --label ticket --state open \
  --json number,title,labels,assignees,updatedAt --limit 100 \
  | jq -r '.[] | select([.labels[].name] | index("status:ready")) | "\(.number)\t\(.title)"'
```

Anything already claimed: the last `braid` block in the issue's comments is the handoff.
