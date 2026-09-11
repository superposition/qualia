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
