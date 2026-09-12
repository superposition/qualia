# Journal figures

Every journal entry's pictures live here, one directory per entry, so the entries cannot drift apart
in layout, colour or provenance. This file is the convention step 38 fixes;
`crates/gates` (the `qualia-gates figures` subcommand, #238) is its machine check:

```console
$ cargo run --quiet -p qualia-gates -- figures              # the figures of the worktree you run it in
$ cargo run --quiet -p qualia-gates -- figures --root DIR   # a named tree, e.g. another worktree
$ cargo run --quiet -p qualia-gates -- figures --self-test  # the checker's own fixtures, no repository needed
figures: OK (11 entries, 55 figures, 400 KiB budget)
```

## Layout

```text
docs/figures/
  _house.py                  the palette and the box/arrow vocabulary, shared by every figure
  <entry-slug>/
    make_figures.py          the generator: `python make_figures.py` rewrites the figures below
    README.md                the table, the data, and how to regenerate (the shape below)
    <name>.svg  <name>.png   one figure: vector source, and the raster beside it
    <name>.mmd               a Mermaid diagram (rule 1); source, not a counted asset
    <name>.json              the recording the figure draws (optional)
    <name>.glb               a Blender render (optional; steps 42–43)
```

`<entry-slug>` is the slug the entry publishes under, so the figure path and the entry URL agree:
`the-licence-and-the-snapshot`, `the-operating-model`. A figure set that belongs to a
ticket rather than to a published entry takes a short slug for that set — `fly-brain` is T51's.

## The rules

1. **One chart and one drawing.** Every entry ships with at least one **chart** (matplotlib) and at
   least one **diagram or render**: a schematic drawn in `_house.py`'s box/arrow vocabulary, a
   Blender render from steps 42–43, or **Mermaid** — a `<name>.mmd` committed beside the figures,
   whose `mermaid` block the published entry carries (or a fenced block written straight into the
   entry, with the `.mmd` as the source of record here). Mermaid is sanctioned because GitHub renders
   the fenced block on the repository page and the diagram stays text, so it can be edited and
   reviewed like any other file; it draws in the viewer's theme rather than the house palette, so the
   hand-drawn schematic stays the form where the subject needs the box/arrow vocabulary or the house
   colours. A human learner needs the picture as well as the number, and the teaching editor's
   checklist refuses an entry with only one of the two.

   The machine check reads this tree, not the published entry. A `.mmd` named in the entry's figure
   table is held to the table's own rules — the row's Kind must be one of the known kinds
   (`diagram`), and the file must exist beside the README — but it is not an asset: it is not
   counted in the `figures: OK` total, it is not budget-checked, and the check does not render
   Mermaid, exactly as it does not run `make_figures.py`. A diagram that lives only inside the
   published Markdown is invisible to the check, which is why the `.mmd` is committed here.
2. **The mesh band only where the subject is a numeric field.** The render band is an extra, not a
   default: it is added to an entry whose subject is a numeric field (steps 42–43 own the asset), and
   an entry that is a story with a chart and a schematic does not get one.
3. **One palette.** Background `#101217`, ink `#edf0f5`, accents `#91dbba`, `#c9b2ff`, `#93caff`,
   `#e0a08a`. They are defined once, in [`_house.py`](_house.py), and figure scripts import them from
   there — never retyped, so the entries cannot drift apart in colour. Every committed `.svg` carries
   the background, and the check says so.
4. **Absolute URLs.** The entry references each figure by absolute URL
   (`https://raw.githubusercontent.com/superposition/qualia/main/docs/figures/<entry-slug>/<name>.<ext>`),
   never a relative path. The
   publish gate (`crates/gates`, the `qualia-gates journal` subcommand, step 37b) refuses a
   relative reference; this check makes the same rule true from this side by matching the journal URL
   a directory README quotes to the directory's own slug.
5. **Generated, not pasted.** Each directory carries the generator that drew its figures, and the
   generator imports the house style. The numbers in a figure come from the committed data beside the
   script (a JSON recording, a check's own output), so the figure can only say what the data says.
6. **Deterministic.** `_house.save` suppresses the figure metadata (no date, no creator) and fixes the
   SVG hash salt, so two runs of a generator under the same matplotlib version are byte-identical —
   `cmp` over each `.svg` and `.png` is the check. The one documented exception is Blender's glTF
   exporter (`turntable.glb`): same structure, counts and animation every run, not the same bytes.
7. **Budget.** No figure asset — `.svg`, `.png`, `.webp`, `.glb` — is over 400 KiB, and none is empty.
   Data files are the recording, not the picture, and are exempt from the budget.

## The directory README

The README beside the figures is the table a reader and the check both read. Its first table is the
figure list, with exactly this header and one row per file:

```markdown
| File | Kind | What it encodes |
| --- | --- | --- |
| `provenance-check.svg` (+ `.png`) | chart | For each commit, three bars: ... |
| `licence-flow.svg` (+ `.png`) | diagram | The clean-room boundary ... |
```

The **File** cell is the committed path in backticks — `(+ \`.png\`)` marks the raster beside an SVG.
The **Kind** cell begins with one of `chart`, `diagram`, `render`, `3D asset`, `data`, and may carry a
qualifier after a comma (`chart, two panels`). The **What it encodes** cell is the caption's source: it
says what the figure encodes, not what it looks like.

Below the table, two named sections carry the provenance: what the data is and which commands produced
it, and how to regenerate the figures. Quote the run, not a recollection of it.

## Adding an entry

```console
$ mkdir docs/figures/<entry-slug>
$ # write make_figures.py, `from _house import ...`
$ python docs/figures/<entry-slug>/make_figures.py
$ cargo run --quiet -p qualia-gates -- figures
```

No formatter, no build step: the figures are committed output, and the check is the gate.

## What this does not establish

The check is offline and deterministic. It does not fetch the live entries or the figure URLs, so it
cannot say a figure returns 200; that leg is the publish gate's, which resolves the live entry and
every absolute figure URL
(`cargo run --quiet -p qualia-gates -- journal --pr <n> --repo superposition/superposition.github.io --url <url>`).
It also does not judge the figures: whether the chart says the true thing is the accuracy editor's job,
and whether it teaches is the teaching editor's, both from
[`journal-review.md`](../journal-review.md).
