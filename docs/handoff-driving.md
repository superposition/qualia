# Handoff — getting the fly driving the WaveShare

You are taking over a specific, short piece of work: the fly's wheel commands already reach a transport that
can drive the robot; the three demonstrations that prove it are owed, and one small shim is missing between
the fly and the transport. Read `docs/agents.md` (the operating model: breadcrumbs, claims, the board lease,
the definition of done) and, if you are running in this session's harness, the batch context file
`local://qualia-swarm-context.md`. Then read ticket **#262 (T67)** and its review trail — that is the state,
not this file.

## The goal

Run the fly's wheel commands all the way to the wheels, in one bounded window with the operator present, and
prove it from the robot's own evidence. The ticket's acceptance names the three demonstrations:

1. **A bounded live run** at `speed_mode: low` with our frames applied through the transport, quoting leash's
   own `GET /action-evidence` and `GET /evidence/action/applied` for the applied left/right and the mode.
2. **The deadman expiry**: the command stream stops and the wheels stop within the expiry, with the telemetry
   quoted on both sides of the stop.
3. **The estop latch/refusal/resume** — only if the operator has decided the latch should be clearable (see
   the asymmetry below).

## The blocker you will meet first

As of this handoff, **the robot refuses every drive command, zero speed included**:

```text
POST http://192.168.55.1:8000/motors/drive refused HTTP 400:
{"error":"runtime v2 Waveshare acknowledgement timed out","ok":false}
```

while `/motors/stop/verified` is acknowledged every time and leash's ledger reads `sent=received=205891,
write_failures=0, connected=true, last_stop_receipt.verified_zero=true`. The operator says this is a **known
physical condition they are fixing**, and they will say when to retry. **Do not command motion until they
say so.** Before you retry, a zero-speed `drive` is the cheapest probe: if it is still refused, stop and
report the line; if it is accepted, the window is open.

## What exists

| Thing | Where | State |
|---|---|---|
| The transport | `runners/leash-transport` | Merged (#264, `9515843`; merge `c3b2240`). Reads one command per line on **stdin** in `runners/drive`'s own wire frame: `{"T":41,"L":0.16,"R":0.16}` |
| Its evidence | `docs/evidence/T67/leash-drive/` | Four verbatim run logs, leash's applied-action page over the window, the runtime-v2 status, the journal denials, the token-absence proof, the README |
| The MCP client it shares | `runners/leash-sensors` | `tools/call` envelope, timeouts, the harness's own refusals surfaced |
| The motor path that cannot open the port | `runners/drive` | Merged; resolves a `NavGoal` to `DriveCommand::WheelSpeeds`; on Pinkie it exits 2 with `device or resource busy` because leash owns `/dev/ttyTHS1` |
| The fly that produces commands | `crates/connectome-cns` (`qualia-connectome-cns loop … --trace <csv>`) | Merged. Its live camera loop produced the commands (196/200 host ticks; 296/300 and 294/300 on the board) and writes them to a **trace CSV**, not to a wire frame |
| The post about all of this | `docs/figures/the-fly-brain-on-the-robot/` | Merged (#261); subject is the robot and the fly driving it |

### The one missing shim

Implemented by `loop --frames-out -`: one flushed JSON frame per tick, with that tick in `T` and an
explicit zero for every hold. The same decoder values write the CSV. The loop now decodes during
acquisition rather than after the full run, so a camera gap reaches the transport as a gap. Status goes
to stderr. `--max-wheel-speed` defaults to `0.04`; a frame-file destination must be new.

The CSV rates are **neural firing fractions**, not wheel speeds. The bridge maps `command` and
`throttle` into bounded pivots; the mapping and live pipe recipe are in
[`crates/connectome-cns/README.md`](../crates/connectome-cns/README.md#wheel-frames-for-the-leash-transport).
[`docs/evidence/T67/fly-frames/`](evidence/T67/fly-frames/) records the local CPU/GPU checks. No robot
request or fresh acknowledgement probe accompanied them: both the zero probe and motion still wait
for the operator's word to retry. The three demonstrations remain owed.

## Running the transport

```sh
export QUALIA_LEASH_BASE_URL=http://192.168.55.1:8000          # the robot; no tunnel needed on this route
export QUALIA_LEASH_OPERATOR_TOKEN_FILE=<path to the bearer>   # re-fetch from the robot, 0600, then delete your copy
export QUALIA_LEASH_TRANSPORT_ROUTE=http                       # http (default) | mcp (behind the legacy flag)
export QUALIA_LEASH_TRANSPORT_DEADMAN_MS=500                   # the command expiry
export QUALIA_LEASH_TRANSPORT_LEASE_TTL_SECS=20
export QUALIA_LEASH_SPEED_MODE=low                             # start here; the operator raises it
<frames producer> | cargo run -p qualia-leash-transport
```

The bearer lives on the robot at the path its `LEASH_OPERATOR_AUTH_TOKEN_FILE` names (mode 0600). The host
copy from the T67 runs was **deleted**; re-fetch it, and delete it again when you are done. Never let it
reach a log, an evidence file, or a commit — `git grep -F -f <bearer> -- .` must be empty.

What the transport guarantees, and what you must not break: every failure ends in **zero speed or a hold**
(unreachable base URL, HTTP error, timeout, malformed frame, refused token, a non-live or unarmed harness,
deadman expiry, an unacknowledged zero); the deadman's clock runs from the **frame's arrival**; a verified
stop **ends the pilot lease**, so a fresh `authorize` precedes the next command; and the session label is one
stable value per run so the log can be correlated.

## The robot's control surface (measured live)

`POST /pilot/authorize {token, ttl_secs, speed_mode}` → a pilot lease; `POST /motors/drive
{token,left,right,speed_mode,approval}` → the wheels (ungated beyond the bearer); `POST /motors/stop/verified`
→ a verified zero; MCP `tools/call stop`; `GET /health`, `/capabilities`, `/runtime-v2/status`,
`/action-evidence`, `/evidence/action/applied`. The adapter is `profile: waveshare-ugv`, `category:
mobile-base`, `maturity: alpha`, with gates `physical-actuation` and `policy-token-or-approval`, and a human
dashboard at `http://192.168.55.1:8000/dashboard`.

**The estop asymmetry — read this before you touch an estop**: `POST /estop/reset` (and the dashboard's
`estop-reset`) sits behind `LEASH_ALLOW_LEGACY_PHYSICAL_CONTROL`, the same flag that gates the MCP
`invoke_capability` drive path. On the current deployment the flag is unset, so **the network can latch an
estop and cannot clear it**. Do not latch one unless the operator has decided to make it clearable; the T67
run measured the reset's refusal with nothing latched rather than stranding the robot, and that was right.

## Rules you must keep

- **No motion without the operator present**, `speed_mode: low` first, bounded duration, then stop.
- One instance per host; no windows on the operator's desktop (D-023); the board is a lease (D-024) — but
  nothing here needs the board.
- One build at a time, `-j 2` (D-014). If you build with an out-of-tree `CARGO_TARGET_DIR`, the gates now
  handle it (#266): an unresolvable root exits 2 and an empty comparison exits 1, so a green means something.
- Checkpoint every ~20 minutes: commit, push, post a braid on #262 with `next:` as one imperative action.
- The gates: `cargo run --quiet -p qualia-gates -- provenance|figures|journal|mission`.

## What "done" looks like

The three demonstrations quoted from leash's own evidence, the shim committed with the fly's trace as its
input, and #262 closed by its acceptance rather than by a commit message. If the acknowledgement is still
refused when you start, the honest deliverable is the shim plus the probe's refusal line — say so on the
ticket and leave it open for the operator's word.
