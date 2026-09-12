# T67 — our wheel commands, applied through the leash

Ticket [#262](https://github.com/superposition/qualia/issues/262). This directory is the audit the
ticket asks for: the transport that carries `runners/drive`'s wheel commands to the robot's actuator
(which the leash owns), the live runs against the robot, and leash's own record of what was applied.
It is **not a profiler capture** — no CUDA kernel runs here, so it commits no `capture.json`, no
`kernels.json` and no `kernels.csv`
([`docs/evidence/README.md`](../../README.md) §Layout, the `T<NN>/<slug>/` form).

Branch `ticket/T67-leash-drive` at the commit this README lands with. The change is
`runners/leash-transport` (the transport) and `runners/leash-sensors` (`call_with`, and the harness's
own refusal surfaced from a non-2xx reply).

## The topology the runs used

The robot's harness is the leash on Pinkie: `systemctl --user` unit `leash.service`,
`EnvironmentFile=%h/.config/leash/leash.env`, `ExecStart=%h/.local/bin/leash serve http`, pid 1299,
`mode: live`, `physical: true`, profile `waveshare-ugv`, role `pinkie`, `LEASH_DEADMAN_MS=400`,
`LEASH_POLICY_MODE=require-token`, `LEASH_DRIVE_INVERT=true`, both serial ports held (`ttyTHS1` for
the drive, `ttyACM0` for the LD06 — D-025).

The transport ran on the **dev host**, against the robot's leash over the network:

```bash
QUALIA_SHM_NAME=/qualia_t67 \
QUALIA_LEASH_BASE_URL=http://192.168.55.1:8000 \
QUALIA_LEASH_OPERATOR_TOKEN_FILE=<the operator bearer file, mode 0600, outside the tree> \
QUALIA_LEASH_TRANSPORT_DEADMAN_MS=1000 \
QUALIA_LEASH_TRANSPORT_HEALTH_MS=500 \
./qualia-leash-transport.exe
```

with one wheel command per line on stdin, in the frame `runners/drive` writes to the wire:

```bash
printf '{"T":1,"L":0.0,"R":0.0}\n'
```

The arena was a private `init`-owned region (`QUALIA_INIT_OWNER_ONLY=1`, `/qualia_t67`, manifest with
`stack_name`/`shared_memory.name`/`control.socket` renamed) so the operator's live `/qualia_body`
stack was untouched.

**Which peer, and why it mattered.** On the `http` route the transport needs no loopback peer: the
leash's `operator_lease_issuer_allowed` accepts a remote peer that presents the bearer secret from
`LEASH_OPERATOR_AUTH_TOKEN_FILE`, and that is what every run here did (host → `192.168.55.1:8000`).
The loopback gate in `44fa250/src/http.rs:3723` is the *legacy* path's, and it applies to the MCP
`invoke_capability` route; the MCP run below therefore did not satisfy it either — the flag gate is
checked first, so the refusal it recorded is the flag's. The MCP route needs both the flag and a
loopback peer (an `ssh -L` tunnel from the host to the robot's `127.0.0.1:8000`, or the binary run on
Pinkie), and neither was enabled, by the operator's instruction.

## What the transport does

| Behaviour | How it is carried |
| --- | --- |
| Wheel commands | One line per command on stdin, `{"T":<tick>,"L":<left>,"R":<right>}`, applied through the leash's `drive` under a pilot lease |
| Authorize | `POST /pilot/authorize` with the bearer secret, `{token, ttl_secs, speed_mode}`, refreshed at half its TTL; the session token is minted here and is *not* the bearer (leash refuses that: "pilot session token must be distinct from the operator bearer token") |
| Speed mode | `QUALIA_LEASH_SPEED_MODE`, default `low`, sent on `authorize` and on every `drive`, and in the log line with the leash's own `max_speed` |
| Deadman | Every applied command carries the expiry `QUALIA_LEASH_TRANSPORT_DEADMAN_MS` (default 500); the transport waits only that long for the next one and sends zero speed on expiry, with the age of the last command in the log line |
| Transport error | A refused or unreachable `drive` is followed by a zero-speed stop, and a stop the leash does not acknowledge sets a **hold**: no command is applied until a stop is acknowledged |
| `estop` | Read from the leash's own `health`; while it reports `estop: true` the transport applies nothing and says so, and it resumes when the harness reports the reset |
| The record | The published arena interval copies the leash's `left`/`right`/`ok`/flags (`ACTION_AUTHORITY_LEASH`); the authoritative record is leash's `GET /action-evidence` and `GET /evidence/action/applied`, quoted below |

`QUALIA_LEASH_TRANSPORT_ROUTE=http|mcp` selects the envelope. `http` is the default because it is
what the deployed leash serves; `mcp` is the ticket's `tools/call` interface, and a `drive` there is
`invoke_capability {capability:"drive", left, right, token, speed_mode, approval}`.

## The runs

Four runs, all against the real harness; the logs are committed verbatim next to this README.

| Log | Transport | Leash | What it shows |
| --- | --- | --- | --- |
| [`transport-http-live.log`](transport-http-live.log) | `http`, host | `192.168.55.1:8000`, live | a command applied and refused by the runtime, zero sent, then 261 acknowledged stop intervals at ~10 Hz |
| [`transport-run.log`](transport-run.log) | `http`, host, private arena | live, with a real link hiccup | the hold: a stop that is not acknowledged refuses commands; the hold releases when a stop is acknowledged; then a refused drive and an unreadable frame both send zero |
| [`transport-unreachable.log`](transport-unreachable.log) | `http`, host, base URL on a closed port | none | every stop fails → `holding, no command will be applied`, and the command is refused without a `drive` being attempted |
| [`transport-mcp.log`](transport-mcp.log) | `mcp`, host | live | the ticket's `tools/call` interface measured: `stop` carries, `drive` is refused in the harness's own words |

`transport-mcp.log`, `transport-run.log` and `transport-unreachable.log` were captured at `3512198`;
they differ from the head only in that their refusal lines do not yet carry the run's session label,
which the last commit added. They are otherwise verbatim.

The zero-speed demonstration, in the transport's own lines (this pair is
[`transport-http-live.log`](transport-http-live.log), captured at the head this PR carries; the same
label `session=c7428df8…` is on both lines because it is derived once per run):

```text
qualia-leash-transport: carrying our wheel commands to http://192.168.55.1:8000 over http; deadman=1000ms speed_mode=low lease_ttl=20s session=c7428df8… token_file=… authority=leash arena=/qualia_t67
qualia-leash-transport: drive refused (T=1 left=0.000 right=0.000) session=c7428df8…: POST http://192.168.55.1:8000/motors/drive refused HTTP 400: {"error":"runtime v2 Waveshare acknowledgement timed out","ok":false}; sending zero
qualia-leash-transport: leash acknowledged the stop: zero command confirmed
qualia-leash-transport: idle 60 stop intervals; estop=false accepted=61 refused=0 failures=0
```

and in the harness's own words over the ticket's interface:

```text
qualia-leash-transport: drive refused (T=11 left=0.000 right=0.000): invoke_capability refused HTTP 403: -32001 legacy physical control is disabled; enable it explicitly at deployment; sending zero
```

### leash's own record

`GET /evidence/action/applied` (`leash.applied-action-page.v1`) over the final run's window — the
committed page is 300 intervals, `action_sequence` 1015451–1015750, `interval_start_ns`
1789222026.45–1789222046.94 (14:07:06.45–14:07:26.94Z; 270 of them inside 14:07:00–14:07:25Z);
`[applied-action-window.txt](applied-action-window.txt)`:

```text
seq=1015451 t_ns=1789222026452411400 req=0,0 app=0,0 scale=0.22 flags=16 valid=true armed=false
seq=1015452 t_ns=1789222026552827300 req=0,0 app=0,0 scale=0.22 flags=16 valid=true armed=false
…
seq=1015750 t_ns=1789222046943666700 req=0,0 app=0,0 scale=0.22 flags=16 valid=true armed=false
```

Every one of the 300 rows carries `req=0,0 app=0,0 scale=0.22 flags=16 valid=true armed=false`:
`scale=0.22` is the leash's `low` ceiling (`max_speed: 0.22` in the stop reply) and `flags=16` is
`LEASH_ACTION_SAFETY_VERIFIED_ZERO` (`crates/types/src/lib.rs:502`), so every interval the leash
kept in that window is a **verified zero**, and no interval in it carries a non-zero request —
because no drive was accepted. The transport's own log for the same seconds carries the hold that
the link hiccup set (`transport-run.log`: `accepted=0 … failures=1`, then `accepted=19` after the
stop is acknowledged again).

`GET /runtime-v2/status` ([`runtime-v2-status.json`](runtime-v2-status.json)):

```json
{"available": true, "motor_authority": "waveshare-controller-owner", "control_authority": "cpu-safety-supervisor",
 "connected": true, "estopped": false, "last_error": null,
 "metrics": {"accepted": 205891, "disconnects": 0, "malformed_telemetry": 240, "reconnects": 1,
             "rejected_estopped": 0, "rejected_full": 0, "superseded_by_safety": 27,
             "telemetry_frames": 1016146, "worker_panics": 0, "write_failures": 0, "writes": 210846},
 "command_lane": {"capacity": 16, "depth": 0, "dropped": 0, "received": 205891, "rejected": 0, "sent": 205891},
 "last_stop_receipt": {"kind": "stop", "applied_sequence": 210845, "coalesced": 4980,
                       "first_request_sequence": 1, "through_request_sequence": 4981, "verified_zero": true}}
```

The command lane is healthy, the controller is connected, the stop receipt is a verified zero — and
the drive acknowledgement is what is missing.

The harness's journal records each refusal with its own reason
([`leash-journal-denials.txt`](leash-journal-denials.txt)). The committed page holds **20 denials
between 13:51:29Z and 14:09:17Z: 14 of them `reason="runtime v2 Waveshare acknowledgement timed out"`
and 6 `reason="invalid pilot token"`.** The two classes are different things and the distinction is
part of the lease finding below — the timeout is the drive path refusing the command, the invalid
token is a lease that a stop had already revoked:

```text
2026-09-12T14:03:22.110521Z  WARN leash_harness::capability: capability policy denied capability="drive" safety="physical-motion" origin="operator-http" policy_mode="require-token" reason="runtime v2 Waveshare acknowledgement timed out"
2026-09-12T13:51:33.662290Z  WARN leash_harness::capability: capability policy denied capability="drive" safety="physical-motion" origin="operator-http" policy_mode="require-token" reason="invalid pilot token"
```

## The environmental state: the drive acknowledgement

`POST /motors/drive` is refused **for every command, zero speed included**, with
`{"error":"runtime v2 Waveshare acknowledgement timed out","ok":false}` (HTTP 400): **14 refusals of
that reason between 13:51:29Z and 14:09:17Z** in the committed journal page, while
`POST /motors/stop/verified` is acknowledged every time
(`acknowledged: true`, `"zero command confirmed"`) and MCP `stop` answers `ok: true` with
`max_speed: 0.22`. The operator's answer to this audit: **the acknowledgement timeout is a known
physical condition they will fix, and they will say when to retry.** No moving command was attempted,
and none should be until then. `[leash-refusals.txt](leash-refusals.txt)` is the request/response
transcript of every surface, with the bearer redacted.

## A stop ends the lease

Measured cleanly (no transport running; `[lease-lifetime.txt](lease-lifetime.txt)`):

| Step | Reply |
| --- | --- |
| `POST /pilot/authorize {ttl_secs:30, speed_mode:low}` | `{"ok":true,"ttl_secs":30,"speed_mode":"low"}` |
| `POST /motors/drive` (zero) | `{"error":"runtime v2 Waveshare acknowledgement timed out","ok":false}` — the lease carries |
| `POST /motors/drive` again, no stop between | `{"error":"runtime v2 Waveshare acknowledgement timed out","ok":false}` — still carries |
| `POST /motors/stop/verified` | `{"acknowledged":true,"statement":"zero command confirmed"}` |
| `POST /motors/drive` after the stop | `{"error":"invalid pilot token","ok":false}` — **the stop revoked it** |
| `POST /pilot/authorize` again | `{"ok":true,"ttl_secs":30}` |
| `POST /motors/drive` | `{"error":"runtime v2 Waveshare acknowledgement timed out","ok":false}` — carries again |

The transport therefore re-registers a lease after every zero, so the command after a stop is not
refused for a reason the runner could have avoided.

## `estop`: why it was not latched

The transport's estop handling is read from the leash's `health` and needs no latch to be
implemented, but the *demonstration* — latch, watch the transport refuse, reset, watch it resume —
needs a reset path, and on this deployment there is none:

```text
== http POST /estop/reset, with no estop latched: the gate is checked first ==
{"error":"legacy physical control is disabled; enable it explicitly at deployment","ok":false}
== the dashboard records the refused reset (leash' own event line) ==
estop_reset error legacy physical control is disabled; enable it explicitly at deployment
```

`44fa250/src/http.rs:3621-3624` puts `require_legacy_physical_control` in front of both
`/estop/reset` and `/dashboard/estop-reset`, i.e. the same `LEASH_ALLOW_LEGACY_PHYSICAL_CONTROL` +
loopback gate as the MCP drive. So the network can latch this robot's emergency stop and cannot
clear it. Latching it to demonstrate the refusal would have left the robot stopped until a leash
restart or a physical reset, so the demonstration was **not performed**; the refusal of the reset was
measured instead, with no estop latched. Note that this probe left one line in the operator's
dashboard event list, and that it is the truthful record of a refused reset.

What *is* demonstrated for the stop path is the hold: a stop the leash does not acknowledge refuses
commands (`transport-unreachable.log`, `estop=false hold=true; refused T=7 …`), and it releases when
a stop is acknowledged (`transport-run.log`, `the stop is acknowledged again; holding released`).

## The token

Read from the file `QUALIA_LEASH_OPERATOR_TOKEN_FILE` names (the same key `runners/agent` and
`config/stack-manifest.default.json` already use for this secret), trimmed, never logged, never
written to the arena, and not sent as the session token (leash refuses that case explicitly). The log
carries the file's *path* and one eight-hex **session label**: a digest of the session token, derived
once at startup and stored, so every line of a run carries the same value and a session can be
followed through the log — it names the session without being it. Proof that the secret itself is
absent, over every tracked file and over this directory:

```console
$ tr -d '\r\n' < <bearer file> > /tmp/token-oneline.txt && wc -c /tmp/token-oneline.txt
64 /tmp/token-oneline.txt
$ git grep -F -f /tmp/token-oneline.txt -- . ; echo "GIT_GREP_RC=$?"
GIT_GREP_RC=1
$ grep -F -f /tmp/token-oneline.txt -r docs/evidence/T67 ; echo "FILE_GREP_RC=$?"
FILE_GREP_RC=1
```

## What is not done

* **The moving-command run.** Blocked by the drive acknowledgement above; it needs the operator's
  word. The path is ready: a producer writes `{"T":…,"L":…,"R":…}` frames into the transport's stdin
  (`runners/drive`'s own wire frame; the connectome loop's left/right decision carries the same two
  numbers), `speed_mode` starts `low`, and the run is bounded by the deadman.
* **The deadman's expiry, shown live.** The branch is implemented and arms on every applied command,
  and it logs the age of the last command when it fires — but it can only fire after a command the
  leash *accepted*, and this harness accepts none. What the runs show instead is the stronger
  envelope around it: every non-accepted path sends zero within one poll, and the quiet stream is a
  continuous verified zero at 10 Hz.
* **The `estop` demonstration.** Needs a leash that can clear it (see above).

## Commands run

```console
$ cargo build -j 2 -p qualia-leash-sensors -p qualia-leash-transport      # BUILD_RC=0
$ cargo run -j 2 --quiet -p qualia-gates -- provenance
provenance: compared 152 authored file(s) (1 generated skipped); 0 identical, 27 EOL-identical, 7 code runs over 20, 0 prose runs over 2
provenance: highest identical-code-line share 1.000 (.cargo/config.toml)
provenance: OK                                                            # GATE_RC=0
```

Run the gate **from the worktree with the worktree's own target directory**. `cargo-gates`'s
`cwd_root()` falls back to the directory holding the binary when the running binary's
`--git-common-dir` does not match the checkout's, so a `CARGO_TARGET_DIR` outside the tree makes the
gate compare **0 authored files** and still print `provenance: OK` — a pass that checked nothing.
That is how an earlier run of this audit quoted `compared 0 authored file(s)`; it is corrected here,
and it is worth its own ticket rather than a note.

On Pinkie, the harness's own surfaces (all read-only except the two stops and the one refused
`estop_reset` probe):

```bash
curl -s http://127.0.0.1:8000/health
curl -s http://127.0.0.1:8000/capabilities
curl -s http://127.0.0.1:8000/runtime-v2/status
curl -s "http://127.0.0.1:8000/evidence/action/applied?after_sequence=<n>&limit=200"
curl -s http://127.0.0.1:8000/action-evidence
curl -s -X POST http://127.0.0.1:8000/mcp -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"stop","arguments":{}}}'
curl -s -X POST http://127.0.0.1:8000/pilot/authorize -H "authorization: Bearer <bearer>" -d '{"token":"<session>","ttl_secs":20,"speed_mode":"low"}'
curl -s -X POST http://127.0.0.1:8000/motors/drive -H "authorization: Bearer <bearer>" -d '{"token":"<session>","left":0,"right":0,"speed_mode":"low","approval":true}'
curl -s -X POST http://127.0.0.1:8000/motors/stop/verified -H "authorization: Bearer <bearer>" -d '{"reason":"operator-request"}'
journalctl --user -u leash --no-pager -o cat --since "2026-09-12 09:45"
```
