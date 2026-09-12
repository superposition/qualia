# qualia-mission-broker

The mission broker: the producing side of the mission wire. It watches the
braid's proposals, asks the coach — an OpenAI-compatible chat surface, DeepSeek
by default — for one `CoachDecision` per item that needs one, and publishes the
resulting `MissionEnvelopeV1` to the agent's operator surface with the bearer
scope the agent already defines.

```text
proposal + braid state ──> coach ──> CoachDecision{kind, rationale, evidence}
                                        │
                            promote ────┴──> promote_proposal_to_canonical (the
                                             repository's own rule: exactly this
                                             proposal, never a planner advisory)
                                        │
                            mission draft ──> MissionEnvelopeV1 ──> POST
                                             /mission-control/envelopes
```

Nothing in this crate invents a value the agent would have to trust:

- the decision is built as `qualia_sync_types::CoachDecision` and checked by
  `validate_shape`;
- a `promote` is materialized through `promote_proposal_to_canonical`, so a
  target that is not the proposal in hand, or a planner advisory, is refused;
- the mission envelope's evidence is the proposal's own
  `lineage.evidence_refs` — a promote with no lineage evidence opens no mission,
  because a `start` without evidence is not one the agent accepts;
- the envelope is checked by `MissionEnvelopeV1::validate` before it is posted.

The numeric envelope (speed ceiling, distance, runtime, replans, area) is the
**broker's**, not the model's: the model chooses the decision and the objective,
and the broker keeps the robot inside the contract the agent enforces. Defaults
are `config.rs`, and a model value outside the agent's bounds is refused with
the reason rather than clamped.

## Running it

```console
# one tick — the acceptance path; prints what it posted where
qualia-mission-broker --once --proposal docs/evidence/T63/proposal-frontier-1.json

# a bounded multi-tick run, with the console's status surface up
qualia-mission-broker --ticks 30 --poll-ms 1000 --proposal <path>
```

`--proposals` (or `QUALIA_MISSION_BROKER_PROPOSALS`) names the braid's proposal
record as JSON: `{"proposals": [...]}`, a bare array, or one object. When it
names nothing, the broker reads `GET /world-model/proposals`; in this build
that route answers `503` because the spatial world model is not in the agent
yet (`runners/agent/src/pending.rs`), and the broker says exactly that instead
of inventing an empty list.

## Environment only

| key | default | meaning |
| --- | --- | --- |
| `QUALIA_AGENT_URL` | `http://127.0.0.1:8080` | the agent's operator surface |
| `QUALIA_AGENT_TLS_DIR` (`QUALIA_TLS_DIR`) | — | the agent's `cert.pem`, added as a root; no insecure bypass |
| `QUALIA_MISSION_BROKER_TOKEN` / `..._TOKEN_FILE` | — | the mission-broker bearer |
| `QUALIA_MISSION_BROKER_ID` | `qualia-mission-broker-<host>` | the `broker_id` in every envelope |
| `DEEPSEEK_API_KEY` / `QUALIA_COACH_API_KEY` (or a `_FILE` path) | — | the coach's credential |
| `QUALIA_COACH_BASE_URL` | `https://api.deepseek.com` | the OpenAI-compatible base |
| `QUALIA_COACH_MODEL` | `deepseek-chat` | the model |
| `QUALIA_COACH_TIMEOUT_MS` | `8000` | a decision that does not arrive degrades to no decision |
| `QUALIA_COACH_STATUS_PORT` | `8091` | the console's Coach panel surface, loopback only |
| `QUALIA_COACH_TICK_MS` | `1000` | gap between ticks |
| `QUALIA_MISSION_BROKER_FLY_GOVERNED` | `false` | the envelope's `fly_governed` |

The credential is never a file in this repository and never reaches a log line,
an error path, a status payload or an evidence file. Redaction is
`redact.rs`: any marker-shaped run becomes `[redacted]`, and the quoted prefix
is four characters of key material after the provider's three-character prefix,
with the length stated — `1448…(redacted, 35 chars)`. The literal shape a key
has is deliberately absent from this tree, because the acceptance runs
`git grep` for it and must find nothing.

## Honest degradation

A run with no credential, or one whose coach does not answer inside the
timeout, produces **no decision**: one named line, no envelope, and
`llm_priors_ablated: true` with the reason. A decision a model did produce
records `false` plus the model id, the prompt digest (SHA-256 over the exact
request body), the response id, the prompt/completion token counts — or that
the provider returned no usage — and a wall-clock stamp. A provider that
answers with something that is not a usable decision object is the
`unparseable` degradation, not a decision.

`llm_priors_ablated` is `false` exactly when a model produced the decision.

## The bearer is required, measured

`AuthScope::MissionBroker` is excluded from the loopback bypass on purpose
(`runners/agent/src/auth.rs:76-80` — only `Read`, `Peer` and `Compute` get it),
so a loopback caller must still present the bearer. Measured against the
from-`main` agent on `https://127.0.0.1:8080`:

```text
POST /mission-control/envelopes with a valid body and no Authorization header -> HTTP 401
POST /mission-control/envelopes with a valid body and a wrong bearer          -> HTTP 401
```

One caution for the next agent: a body that does not deserialize answers `422`
**before** the handler runs, because axum runs the `Json` extractor ahead of the
handler's own `authorize` call. A `422` therefore says nothing about the bearer
either way; only a request with a valid body exercises the auth check.

## The status surface is a second source

`GET http://127.0.0.1:8091/coach` (`qualia.coach-state.v1`) is what the
operator's console Coach panel reads: the model's state (`ok` / `no_key` /
`timeout` / `error`, model id, last latency, last error) and the newest
decisions with their provenance. It is a read-only `std::net` listener on a
loopback address — a non-loopback host is refused rather than served — and it
exists because the agent owns missions and has no coach stream today.

**This is deliberately a second source beside the agent.** The durable home of a
mission is the agent's mission control; the durable home of a decision should be
the agent's world-model surface (`/world-model/decisions`), which is a `503`
stub in this build. When the agent grows a coach/decision stream, the console
should read that and this endpoint should be retired; until then the console
reads the broker directly, and the console's README records the same direction
so a later agent does not find two ways to see the same thing without knowing
which one is temporary.
