# T63 (#244) — the mission broker speaks to the model

Evidence for the coach client and the broker's first live runs. Every run below
is the committed tree; no fixture stands in for the provider or the agent, and
no window was opened on the operator's desktop (D-023 — the console figure is
rendered headlessly).

## What ran

| Step | Command | Result |
| --- | --- | --- |
| build | `cargo build -p qualia-mission-broker -j 2` | `Finished dev profile … in 53.05s` |
| no-key pass | `qualia-mission-broker --no-status --once --proposal docs/evidence/T63/proposal-frontier-1.json`, key unset | exit 0, `state=degraded` |
| live pass | same binary, `--once`, `DEEPSEEK_API_KEY_FILE` from `omp token deepseek` (environment only) | exit 0, `state=posted` |
| bounded run + capture | `--ticks 30 --poll-ms 2000` with the status surface, then the console's headless figure | exit 0, figure written |
| auth probes | one well-formed envelope, three bearer cases (`https://127.0.0.1:8080`) | 401 / 401 / 202 |

The agent is the from-`main` instance on `https://127.0.0.1:8080` (TLS,
`C:/Users/ericm/.qualia_tls/cert.pem`), started by T60 with
`QUALIA_MISSION_BROKER_TOKEN_FILE=C:/tmp/qualia-mission-broker.token`. The
broker read the same secret through the same `_FILE` key.

## The live request and response

`live-run.txt` is the run verbatim; the facts, redacted (the credential never
appears, and this tree contains no key-shaped literal at all):

```text
coach request  model=deepseek-chat base_url=https://api.deepseek.com key=1448…(redacted, 35 chars) prompt_digest=sha256:471331be… prompt_bytes=2859 timeout_ms=8000
coach response id=14e8c6f7-8f7d-472d-a537-30415f7e20ab model=deepseek-flash latency_ms=435 usage=prompt_tokens=775 completion_tokens=144 finish_reason=stop
```

The provider reported its own model id (`deepseek-flash`) rather than the
requested `deepseek-chat`; the broker records what the provider said, in the
decision row and in the console panel. `latency_ms` is wall clock around the
request; `prompt_digest` is SHA-256 over the exact encoded request body.

The model's answer, which the broker parses as a JSON object — no prose is
parsed into a decision:

```json
{"decision_kind":"promote","target_proposal_ids":["proposal-frontier-corner-a"],
 "output_ids":["proposal-frontier-corner-a"],
 "reason":"Frontier region proposal with fresh pose/lidar evidence, high mission relevance, and a bounded reachable centroid inside the operating area justifies one bounded exploration mission.",
 "mission":{"objective_kind":"explore_frontier",
            "summary":"Explore frontier region near corner A within bounded distance and runtime.",
            "target_x_m":1.9,"target_y_m":1.1,"tolerance_m":0.5,
            "max_distance_m":2.0,"max_runtime_ms":60000,"max_replans":1}}
```

The promote was materialized by `promote_proposal_to_canonical` (the repository's
own rule — exactly this proposal, never a planner advisory), which returned
`canonical_id=proposal-frontier-corner-a`; the mission envelope took its evidence
from the proposal's `lineage.evidence_refs` (`evidence_refs=2`).

## The agent accepted it

```text
qualia-mission-broker: posted POST https://127.0.0.1:8080/mission-control/envelopes -> HTTP 202 accepted=true idempotent_replay=false
qualia-mission-broker: read back GET /mission-control/missions -> mission mission-coach-7328529549370528-1 status=paused stage=awaiting_evidence code=awaiting_fresh_evidence
qualia-mission-broker: read back GET /mission-control/events -> seq=1 kind=accepted status=queued code=accepted detail=bounded mission was durably accepted
```

The agent's own answer, captured from `GET /mission-control/missions`
(`missions-after-live-run.json`, read with the CA certificate and no token —
the Read scope is loopback-bypassed):

```text
schema qualia.mission-control-state.v1 broker_epoch 7328531307768660 seq 1 count 2
mission-coach-7328531307768660-1 broker qualia-mission-broker-chalant epoch 7328531307768660 seq 1 cmd start evidence_refs 2 | status paused awaiting_evidence awaiting_fresh_evidence
mission-coach-7328529549370528-1 broker qualia-mission-broker-chalant epoch 7328529549370528 seq 1 cmd start evidence_refs 2 | status failed  terminal           deadline_exceeded
```

The second row is the honest end of the first `--once` pass: its deadline is its
own `max_runtime_ms` (60 s), the mission parked at `awaiting_fresh_evidence`
because this build has no spatial world model (`/world-model/proposals` is a
`503` stub), and the agent's supervisor closed it when the deadline passed. It
never entered motion and no plan exists.

## The console shows the decision

`coach-panel.png` is a headless render — the console's real `render_view`
through `egui_kittest`'s wgpu backend, no window — taken while the broker was up
on its bounded run. The Coach panel reads `GET /coach` and shows, verbatim:

```text
model configured      model deepseek-chat      base url https://api.deepseek.com
key 1448… (redacted, 35 chars)                 last latency 415 ms
decision promote proposal-frontier-corner-a
reason Frontier region proposal with fresh pose/lidar evidence, …
provenance deepseek-flash · tokens 786/164
response 4b17129b-1425-492b-9f72-28ded05b903e
broker posted
mission mission-coach-7328531307768660-1       accepted start HTTP 202
```

and the six views around it (`open_missions=1` on the Mission panel and the
braid session bound to the mission id come from the same agent the mission was
delivered to).

The example prints what it drew:

```text
coach_evidence: broker http://127.0.0.1:8091 -> deepseek-chat (ok), 1 decision(s), 1 mission(s)
coach_evidence: decision coach-7328531307768660-1 promote -> proposal-frontier-corner-a · model deepseek-flash · latency 415 ms · tokens 786/164 · response 4b17129b-… · priors_ablated=false
coach_evidence: mission mission-coach-7328531307768660-1 start accepted=true ack=Some(202)
```

A broker that is not running does not produce an empty panel: the same panel
degrades with `coach broker not running at http://127.0.0.1:8091: <reason>`
(visible in the console's committed `tests/snapshots/mission_degraded.png`).

## The no-key run

`no-key-run.txt`, the same binary against the same live agent with every
credential key unset (the mission-broker bearer is still configured, so the
degradation is the coach's and nothing else):

```text
qualia-mission-broker: agent https://127.0.0.1:8080 token=configured coach model=deepseek-chat base_url=https://api.deepseek.com key=none (llm_priors_ablated=true) timeout_ms=8000
qualia-mission-broker: no coach credential in the environment (DEEPSEEK_API_KEY or QUALIA_COACH_API_KEY); llm_priors_ablated=true, no decision
qualia-mission-broker: tick 1 braid session=mission-coach-7328531307768660-1 generation=0 open_missions=0
qualia-mission-broker: no coach credential in the environment (DEEPSEEK_API_KEY or QUALIA_COACH_API_KEY); llm_priors_ablated=true, no decision for proposal-frontier-corner-a
qualia-mission-broker: run complete ticks=1 items=1 decided=0 posted=0 degraded=2 state=degraded
```

The item was reached and the coach was asked — and no decision was produced, no
envelope was built, and the run exited 0. `llm_priors_ablated` is `true` with the
reason here and `false` on the live run's status payload, i.e. exactly when a
model produced the decision.

## Honest limits

- The proposal the coach decided over is a file
  (`docs/evidence/T63/proposal-frontier-1.json`): the agent's own proposal
  surface is a `503` stub in this build, and the broker reads it when it
  answers. The file's shape is the wire envelope
  (`qualia_sync_types::ProposalEnvelope`, `world.model.v1`) and the broker
  validates it with `validate_shape` on the way in.
- No mission reaches motion: this build has no planner and no Leash evidence
  path, so a delivered mission parks at `awaiting_fresh_evidence` and closes on
  its deadline. The broker's job ends at delivery.
- The coach's numeric envelope is bounded by the broker, not the model (speed,
  distance, runtime, replans, area) — that is deliberate and documented in
  `runners/mission-broker/README.md`.
- The provider returned `deepseek-flash` for a `deepseek-chat` request; the
  broker records the provider's own id rather than the requested one.
