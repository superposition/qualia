# Zero-only live transport attachment

`wheel_transport.py` connects fresh wheel-shadow decisions to the existing
`qualia-leash-transport` binary. It is a **zero-only** integration: any nonzero
frame is explicitly rejected, retained in status, and closes transport stdin.
There is no enable-motion flag and no silent substitution of a nonzero frame.

The bridge does not read credential contents or implement a drive, stop,
authorization, or estop client. The existing transport retains one session,
its arrival-clock 500 ms deadman, low speed mode, 20-second lease refresh,
verified stops, and invalidation of the lease after a verified stop.

## Operator launch

The root operator must first finish the robot deployment, keep the existing
observation arena alive, and provide a protected temporary token-file path in
`QUALIA_LEASH_OPERATOR_TOKEN_FILE`. The bridge passes that environment to the
transport without reading or logging the bearer. Remove the temporary file
after the transport exits. Do not put the bearer in an argument, status,
evidence, or commit.

```powershell
python scripts/wheel_transport.py --transport C:/Users/ericm/qualia/target/debug/qualia-leash-transport.exe --run-dir C:/Users/ericm/.local/state/qualia/console-runtime/wheel-transport-NEW --producer-url http://10.0.0.180:8092/status --base-url http://10.0.0.180:8000 --arena /qualia_observe_20260912 --seconds 60 --port 8093
```

The run directory must be new. Port 8093 is bound to host loopback. A current
zero frame is required before the transport starts; only one owned transport
is launched, with no automatic restart. Root must ensure no other instance
already owns this integration. Do not run a second bridge or transport.

The source is the current HTTP decision, not a historical file tail. Both
publication and original frame-write age must be at most 500 ms, with at most
100 ms of future clock skew; the producer's own decision age and deadline
must also be valid. Frames preserve exactly `T`, `L`, and `R`. A changed run or
backwards tick closes the connection and requires explicit reattachment.

Only one forwarded tick can be outstanding. The bridge waits for the owned
transport's applied or refused record for that exact tick before accepting a
new current tick. This avoids the existing transport's unbounded stdin queue
accumulating historical frames during slow acknowledgements. Ticks not sampled
while awaiting a receipt are not replayed. No new unit-test or build cycle is
required; use the existing binary and one authorized bounded live integration.

## Evidence and status

`GET http://127.0.0.1:8093/status` exposes schema
`qualia.wheel-transport.v1`. `forwarded.jsonl` records actual forwarded frames
with the original producer snapshot and hold reasons; `status.json` retains
the final result after HTTP stops. Source shadow status on 8092 remains
unchanged and correctly describes its own file-only output.

`transport_attached` is measured: the owned process must be alive, its stdin
open, a stable public session label observed, and a forwarded tick consumed
within the last three seconds. It does **not** imply that a drive was accepted.
`transport.last_event` distinguishes `applied`, `drive_refused`,
`locally_refused`, `stop_acknowledged`, and `deadman_expired`. The exact applied
log line is retained because that fixed format contains speeds, flags, and a
public session label only; arbitrary transport diagnostics are not copied into
status. Forwarding and consumption timestamps remain distinct.

The bridge reads the authoritative applied-action page, first obtaining its
latest sequence and then requesting at most eight recent records. The ledger
is explicitly marked `matched_to_transport:false`: global records contain no
session linkage. A record is not assigned to a forwarded tick merely because
it happened nearby in time.

At shutdown the bridge closes stdin, allowing the existing transport to issue
verified EOF stops for five seconds before terminating its owned process.
`final_stop.robot_verified_zero_after_close` requires a fresh valid leash
ledger record with zero applied speeds and the verified-zero flag after the
close time. Its attribution remains **global**, not session-specific. If the
record is unavailable, status says so and the bridge returns failure; it does
not invent a verified receipt. This zero-only connection does not demonstrate
nonzero physical motion, learned driving, or estop recovery.

Prepared for root-controlled live deployment after the Leash compatibility
repair. No transport or actuator request was launched while implementing it.
