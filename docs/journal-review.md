# Journal review checklists

Three editors, three different jobs. Each posts one comment on the entry's PR beginning with a fenced
block:

````text
```braid-review
role: accuracy|teaching|style
verdict: approve|request-changes
notes: <count>
```
````

followed by the completed checklist, copied verbatim from here.

## accuracy

1. Every number traced to a source in the entry itself.
2. Units on every quantity.
3. Any value measured in a different session or on different hardware labelled as such.
4. Every figure's caption says what it encodes.
5. No claim about a private repository.

## teaching

1. The entry explains the idea before the result.
2. At least one chart **and** at least one diagram or 3D render, each captioned so a reader can
   interpret it without the text.
3. Jargon defined on first use.
4. The analogy named where one exists.
5. A reader who stops after the first paragraph still learns something true.

## style

1. The site's form: `**The claim.**` first, then `## What we tried`, `## Evidence`,
   `## What this does not establish`.
2. Short paragraphs, no filler.
3. Front matter valid for the site's build.
4. Figures and band referenced by absolute URL.
