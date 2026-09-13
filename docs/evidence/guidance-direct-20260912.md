# Direct Guidance input, 2026-09-12

The running observer now acquires fresh visual status directly from
`http://10.0.0.180:8091/status`. A bounded three-second acquisition window catches
new publications instead of allowing one expired mirrored sample to suppress the
whole twenty-second advice interval. The original 1500 ms source/output gates
remain. Unavailable input is `waiting_input`; provider failures remain errors.
The console names these separately and retains the timestamp of previous advice.

The provider context includes measured body observations and selected actual
visual cell-type statistics. Compacting it reduced the observed prompt from
approximately 15,000 tokens to approximately 3,300. No API credential is present
in these files. The observer sends advice only: it changes neither weights nor
beliefs and dispatches no wheel or camera command.

Production build: `cargo build -j 2 -p qualia-mission-broker --bin qualia-coach-observer`
passed (18.51 seconds). The installed executable SHA256 is
`88701b01fee935cc11bfc422bbe9e1b0aa791a1531b25abfa074ed61f67f9dc0`.

Live integration after restart at 21:11 EDT: three provider replies were
published, latest latency 1284 ms, status `ok`, `last_error: null`. Replies name
the current robot model run and source tick; one explicitly acknowledged that
minimum lidar range alone does not establish obstacle direction. The local
sanitized capture is `console-runtime/guidance-direct-integration.json`.

This is evidence of successful advisory requests and source handling, not
learning, motor execution, metric VSLAM, or the three driving demonstrations.

A later live integration exposed intermittent direct-HTTP source failures while
an independently fetched mirror was current. The observer now falls back to that
mirror only after the identical source-identity, image-pairing and1500ms age
validation. `acquisition_source` records which path supplied each request; robot
timestamps are retained. Persistent input failure remains `waiting_input`.

The revised broker production build passed as part of the combined console/broker
build. Installed SHA256:
`9529fa08d24fbcff30354c7cb42be4d3ef01634b97827b4525d535fc40176099`.
Live status at21:27EDT held17 actual replies, latest1291ms, status`ok`, and no
current error. This is bounded advisory execution, not semantic-prior dispatch.
