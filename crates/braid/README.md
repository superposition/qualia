# `qualia-braid`

The braid is **one state machine every strand reports through**. Five strands:

1. **Evidence seals** — a sealed MCAP segment is evidence; partials are quarantined, never lost.
2. **Memory records** — sessions, epochs and mission rows live in SQLite, not here.
3. **Gated promotion** — a candidate generation is promoted only when the evidence gates pass.
4. **A mission broker** — missions open, close, and reach the motion authority.
5. **A bounded healing ladder** — drift drives recalibration, rollback, observe-only, safe stop.

`observe(state, event)` is the single place that mutates `BraidState`. It writes nothing itself: the
durable copies stay in MCAP, SQLite and the generation registry, and the braid is a view plus a
dispatch.

This is **one instance** of the pattern, not the pattern's only possible form. A second braid is a
copy of the pattern, not a fork of qualia.
