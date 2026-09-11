# `web/public`

Static assets served by `qualia-agent` from `QUALIA_WEB_DIR`. Plain ES modules and one stylesheet —
no build step, no framework. The page polls `/braid` and renders the braid state, and keeps the last
good state visible when the agent is unreachable rather than showing an empty window.
