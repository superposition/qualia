# `web/public`

Static assets served by `qualia-agent` from `QUALIA_WEB_DIR`. Plain ES modules and one stylesheet —
no build step, no framework. The page polls `/braid` and renders the braid state, and keeps the last
good state visible when the agent is unreachable rather than showing an empty window.

`assets/mark/` holds the psi wordmark and its favicon, installed from the repository's committed mark
so the header glyph and the favicon cannot drift from `assets/mark/psi.json`:

```bash
python assets/mark/install_mark.py --site web/public           # install or refresh
python assets/mark/install_mark.py --site web/public --check   # verify, exit non-zero if stale
```

The script copies `psi.svg` and the 180 px Apple touch icon and rewrites the inline
`<svg class="mark">` glyph between the `mark:start` / `mark:end` sentinels in `index.html` from the
rendered mark.

