# Architecture diagrams

The `.mmd` files in this directory are the source. The `mermaid` blocks in the repository `README.md`
are copies of them, so the repo shows the pictures without a build step.

| File | Shows |
| --- | --- |
| `braid.mmd` | The strands as they are in code, each edge labelled with the type that crosses it. |
| `epics.mmd` | The ticket dependency graph, arrows from blocker to blocked. |
| `agents.mmd` | The operating loop and the failure edge. |
| `states.mmd` | The ticket state machine over the `status:*` labels. |
| `journal.mmd` | The publishing pipeline. |
