# Frozen-vision wheel shadow

`wheel_shadow.py` reads the running pretrained model and actual robot body
measurements, computes an **engineered** wheel proposal, and writes a bounded
JSONL stream. It does not contain an actuator client, acquire authorization,
read a bearer, change model parameters, or attach a transport. This is a
readout from a frozen visual model, not learned driving.

## Run

Python 3.8 or newer, standard library only:

```sh
python3 scripts/wheel_shadow.py \
  --run-dir /home/jetson/qualia-flyvis-20260912/wheel-shadow-NEW \
  --seconds 120 --period-ms 200 --max-speed 0.05 --port 8092
```

The directory and frame file must be new. By default the only frame destination
is `RUN/frames.jsonl`; decisions go to `RUN/decisions.jsonl` and the final
snapshot to `RUN/status.json`. `GET http://ROBOT:8092/status` is read-only and
available during the bounded run. Defaults are a 60-second bound, 200 ms period,
and 0.05 m/s absolute wheel ceiling; the maximum interval is 1800 seconds.

`--frames-out -` emits frames on stdout for a separately authorized live stdin
consumer. It never sends a hardware request itself. Do not replay saved frame
files into an actuator: transport freshness starts at **arrival**, so replaying
an old proposal would lose its original input age. The existing transport owns
deadman expiry, verified stop, lease, speed mode, and acknowledgement handling.

Frames match `runners/leash-transport/src/main.rs`'s `WheelFrame`:

```json
{"T":43,"L":0.0,"R":0.0}
```

`T` is the evidence tick, not a firmware command selector. Each held tick emits
a zero frame; holds are never skipped. Normal completion emits a final zero.
A failed write ends the producer; a nonblocking stdout pipe cannot accumulate
an unbounded backlog. The supervisor terminates a stuck worker at its bound;
an external transport must retain its arrival-clock deadman and EOF stop.

## Inputs and readout

The visual source is `/cells?type=T4a` on port 8091, including all 721 actual
T4a model indices 24039 through 24759. It requires the exact pinned checkpoint
and exported model used by the live observer, a frozen running state, matched
camera/output hash and receipt timestamp, and original V4L2 acquisition/dequeue
metadata. Request, receipt, acquisition, and output must be no more than
1500 ms old; more than 100 ms of future clock skew is refused. Exposure time
remains unknown and is reported as null.

For cell voltage change `dV`, the horizontal centroid is
`sum(abs(dV) * v) / (15 * sum(abs(dV)))`. The actual exported retinal horizontal
axis is `v`; no soma coordinates, spike events, or MaleCNS IDs are invented.
The following gains are engineering choices, not parameters learned by flyvis:

```text
strength = mean(abs(dV)) / (mean(abs(dV)) + 0.002)
forward = 0.025 * (0.5 + 0.5 * strength) / (1 + measured_odom_speed / 0.05)
neural_turn = 0.025 * horizontal_centroid
gyro_damping = 0.008 * measured_gyro_yaw
differential = clamp(neural_turn + gyro_damping, -0.025, 0.025)
L = clamp(forward + differential, -max_speed, max_speed)
R = clamp(forward - differential, -max_speed, max_speed)
```

Positive camera `v` is right. Positive `L-R` is interpreted as clockwise and
positive body gyro yaw as counterclockwise. These are explicit conventions;
this shadow observation does not establish physical steering calibration.
The forward term includes an engineered exploration drive, so a nonzero
proposal is not evidence of a learned motor command. Zero measured neural
change produces a hold.

Body input is `/telemetry/compact` on port 8000. IMU, lidar, and odometry must
all report available, use their expected coordinate frames, be at most 1000 ms
old, and have timestamps within 500 ms of one another. Body data predating the
camera receipt by more than 500 ms is refused. Two advancing odometry samples
establish translation and yaw speed. Holds also cover gyro/odometry yaw
disagreement over 0.5 rad/s, gravity-inclusive acceleration norm outside
4 through 16 m/s², invalid or sparse lidar, and the existing nearest-return
threshold below **0.25 m**. The nearest return is taken across valid beams;
no unsupported forward sector or self-body exclusion is assumed.

The model still takes camera input only. IMU and odometry condition this
external readout, and lidar gates its frame; none is represented as an input
already learned by the frozen vision model. Missing, stale, invalid, or expired
inputs yield a zero frame and explicit reasons. Each decision rechecks its
earliest input deadline immediately before writing a frame.

## Status contract

Schema `qualia.wheel-shadow.v1` reports `run_id`, evidence `tick`, publication
and deadline Unix nanoseconds, `decision_age_ms`, `state`, `hold_reasons`, and
`error`. States are `starting`, `shadow_proposal`, `held`, `stopped`, or `failed`.
`policy_kind` is `engineered_frozen_vision_readout`. In the default file run,
`motion_output` and `transport_attached` are false. Stdout mode reports external
attachment as null because this producer cannot verify its consumer.

`vision` contains the original model run/tick, checkpoint/export hashes, JPEG
hash, acquisition/dequeue/receipt/completion times and ages, real cell type,
horizontal centroid, and mean absolute voltage change. `body` contains sensor
timestamps/ages, gyro yaw, odometry speeds, acceleration norm, and lidar count
and nearest return. `proposed` contains the two proposed wheel speeds plus
`forward_mps`, `neural_turn_mps`, and `gyro_damping_mps`. These three objects
are null until available; a held decision can retain a real proposal while
`emitted_frame` remains zero. `frame_written_unix_ns` identifies the actual
write. After 500 ms without a decision, the HTTP snapshot marks it stale and
preserves the last emitted frame as historical evidence.

## First actual integration

The 2026-09-13 01:04:02 UTC robot run used supervisor PID 67141, a 120-second
bound, and default file output with no transport. Its run ID was
`bde6ff77-32a3-4c23-974a-5133fbd2adb8`; it consumed live frozen vision run
`ca9d4589-fd21-4db8-b05f-62da9769e56a` without restarting it. A saved summary is
in `wheel-shadow-evidence.json`.

The interval completed normally: 595 held decisions plus a final stopped
decision, 596 consecutive frame ticks (0 through 595), and zero nonzero frames.
There were 509 decisions with real neural proposals; the final stopped snapshot
also retained the last proposal as historical evidence. Proposed left speed
ranged from 0.0144827 to 0.0201506 m/s, and right from 0.0131874 to 0.0216142 m/s.
Missing or expired observations produced additional explicit holds. The final
frame was `{"T":595,"L":0.0,"R":0.0}` and the worker reported no error.

At tick 43, the actual neural proposal was L 0.0180885 and R 0.0147482 m/s;
the neural turn term was 0.00165644 m/s and gyro damping 0.0000137392 m/s.
Lidar reported 0.156 m, so the emitted frame was exactly zero. This establishes
live data consumption and the hold path, not successful physical driving,
deadman expiry, or an estop demonstration. The separate zero-speed transport
probe still reported an acknowledgement timeout; repairing that remains an
operator/transport task.

No unit-test cycle, model training, GPU run, wheel/head/light request, or model
replacement was performed for this shadow integration.
