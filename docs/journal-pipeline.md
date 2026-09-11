# The journal pipeline: a writer, three editors, a publish gate

An entry is produced by agents and reviewed before it goes live, and every role's output is a comment
on the entry's PR, so the process is visible and resumable like everything else: a later agent reads
the PR's comments, not another agent's memory. The flow is the one
[`architecture/journal.mmd`](architecture/journal.mmd) draws; this file is the rule the roles follow.

The entry itself is written in the shape [`journal-template.md`](journal-template.md) fixes. The three
editors work from [`journal-review.md`](journal-review.md), which is their source of truth: each
checklist is copied verbatim from it, so a checklist item changes in exactly one file.

The pipeline is the three-reviewer rule of [`agents.md`](agents.md) §Review applied to an entry rather
than to code — three reviewers, three different jobs, each role's last comment its verdict, and no
merge until all three read `approve`.

## The writer

One or two subagents draft the entry for an epic, from primary sources only:

- the epic issue and its ticket comments;
- the `braid` breadcrumbs ([`agents.md`](agents.md) §The breadcrumb) on those issues and PRs;
- the mage captures under [`evidence/`](evidence/);
- `git log` over the epic's commit range.

A writer **may not use its own recollection of the work**, and **may not introduce a number that is
not in one of those sources**. Anything it wants to say that the sources do not support goes into the
draft as a question in an HTML comment:

```html
<!-- ASK: was this reading taken in the same session as the one above? -->
```

An `<!-- ASK: -->` question is not prose and never survives publication. The writer opens the question;
the editors must resolve it or delete it before publishing, and the gate refuses an entry that still
carries one.

The writer opens the draft as a PR against the journal site repository —
`superposition/superposition.github.io`, `_posts/*.md`, Jekyll — based on `main` (D-019), and links
the epic issue and the commit range it covers. **Nothing in this repository pushes to that one**: the
journal site is a handoff, and the work here ends at the *Publish* steps below.

## Three editors, three different jobs

Each editor posts a comment on the entry's PR. Its first line is the fenced block that
[`journal-review.md`](journal-review.md) fixes:

````text
```braid-review
role: accuracy|teaching|style
verdict: approve|request-changes
notes: <count>
```
````

`notes:` is the number of notes the editor raises in the comment. The block is followed by the
completed checklist, copied verbatim from [`journal-review.md`](journal-review.md). A comment carries
exactly one role; a fence that names two roles at once is refused. The three jobs are:

1. **accuracy** — every number traced to a source in the entry itself; units on every quantity; any
   value measured in a different session or on different hardware labelled as such; every figure's
   caption says what it encodes; no claim about a private repository.
2. **teaching** — the entry explains the idea before the result; at least one chart *and* at least one
   diagram or 3D render, each captioned so a reader can interpret it without the text; jargon defined
   on first use; the analogy named where one exists; a reader who stops after the first paragraph
   still learns something true.
3. **style** — the site's form (`**The claim.**` first, `## What we tried`, `## Evidence`,
   `## What this does not establish`), short paragraphs, no filler, front matter valid for the site's
   build, figures and band referenced by absolute URL.

## The review loop

An editor who returns `request-changes` is answered by a revision and **a re-read from that same
editor**: the editor posts again, and **the last comment per role is that role's verdict**. An earlier
comment from the same role is not fatal — the gate reports it as *superseded* and uses the later one —
so an `approve` after a revision opens the gate on the re-read, and a later `request-changes` closes
it. The other two verdicts stand unless the revision touched their concern. Resolving an
`<!-- ASK: -->` is a revision like any other.

## Publish

The author agent merges the entry PR only when all three verdicts are `approve`, then:

1. merges (the merge names the ticket, [`agents.md`](agents.md) §Review);
2. verifies the live URL and every figure returns 200 — each figure by **absolute** URL, because the
   gate refuses a relative reference (style item 4) instead of resolving it;
3. fills the epic's `## Journal` line with that URL.

The gate is machine-checked:

```bash
python scripts/journal_gate.py --pr <n> --repo superposition/superposition.github.io --url <live-url>
python scripts/journal_gate.py --pr <n> --repo OWNER/NAME --comments comments.json --diff entry.diff
python scripts/journal_gate.py --entry _posts/2026-09-11-the-public-record.md
```

The script counts the `braid-review` blocks — the last one per role — checks the three distinct
checklists, and prints `journal-gate: OK` with exit 0 **only when the roles leg was actually
evaluated**: the checklists hold, the roles hold, no `<!-- ASK: -->` survives in the entry, the gate is
open, and, under `--url`, the live entry and every figure URL return 200. It exits non-zero whenever it
cannot open the gate:

- a bare `python scripts/journal_gate.py` and an `--entry`-only run check the checklists (and the
  entry's form) but not the roles, so they print
  `journal-gate: entry OK (roles not checked: pass --pr <n>)` — a bare run says `checklists` where the
  entry run says `entry` — and exit 1; they never print `journal-gate: OK`;
- `--comments` is the PR's comment stream and needs its `--pr <n>`; without it the run is a usage
  error, exit 2, because the JSON would otherwise be silently dropped;
- in a `--url` run a figure referenced by a relative URL is a **style failure**, not a silent skip:
  the gate names the reference and closes.

`--self-test` runs the built-in fixtures — the parsers, the fail-closed exit codes and the re-read
fixtures — with no network and no `gh`. Exit codes are 0 (gate open), 1 (gate not open, the failing leg
named on stderr) and 2 (usage). The ticket's Command is `gh pr view <n> --comments` and count the
`role:` blocks; that display form aborts in this repository on the deprecated `projectCards` GraphQL
field, so the script reads the same comment stream as JSON.

## Resuming

A recovering agent reads the entry PR: the writer's draft and its `<!-- ASK: -->` questions, then one
`braid-review` comment per role — the last one where an editor posted again. The comments are the
state.
