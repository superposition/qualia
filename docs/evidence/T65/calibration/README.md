# T65 calibration — a calibration producer, and the first real session the dataset admits

Step 65 of ticket [#250](https://github.com/superposition/qualia/issues/250). T58
([#239](https://github.com/superposition/qualia/issues/239)) took the whole chain to the robot for the
first time and the dataset leg rejected **all 4 501** candidate transitions of that capture at
`calibration_missing`: no runner in the tree produced the calibration the first gate reads. This ticket
builds the producer, runs a real session through it, and quotes the dataset's own verdict past that
gate: **`valid=1499 candidates=1500`** where the same pipeline reported `0` of `4501`.

The run's own files are beside this README in `board/`, and every number below is a line of one:
`board/logs/calibration.log`, `board/logs/camera.log`, `board/logs/leash-sensors.log`,
`board/logs/leash-transport.log`, `board/logs/pose.head-tail.log`, `board/logs/inspect.log`,
`board/dataset.log`, `board/dataset-manifest.audit.json`, `board/mcap.sha256`, `board/mcap.size`,
`board/rc.txt`. `board/logs/pose.head-tail.log` is trimmed for size (first and last 60 of 3 347 lines,
with the elision marked in the file); everything else is verbatim.

## 1. What the gate reads, and what it takes to pass it

The transition gate is `crates/jepa-dataset/src/lib.rs:511-624`. It forms transitions from **camera
pairs in order** and runs a fixed sequence of checks, each rejecting with its own name in the audit;
the first one that fires decides the ledger, so a ticket that fixes one gate has to know what the next
one will be.

| # | Gate (source) | What it needs | T58's capture | this session |
| --- | --- | --- | --- | --- |
| 1 | `frame_gap` | two frames ≤ 500 ms apart | yes | yes (1 501 camera records at 5.0 Hz) |
| 2 | `camera_invalid` | both frames `valid` | yes | yes |
| 3 | `calibration_missing` | `calibration_id` non-empty, not `"unavailable"` | **no — 4 501 of 4 501** | **yes — `cac618af…`** |
| 4 | `exposure`, `target_exposure` | `luminance_mean` inside 0.05–0.95 | yes (0.437–0.471) | yes (0.381–0.384) |
| 5 | `lidar_missing`, `target_lidar_missing` | a lidar record within 150 ms of each frame | yes | yes (2 917 lidar records) |
| 6 | `pose_missing`, `target_pose_missing` | a `TOPIC_POSE` record within 150 ms of each frame | **no pose stream at all** | yes (2 917 pose records) |
| 7 | `sensor_skew` | the four skews ≤ 150 ms | — | yes: mean 29.19 ms, p95 49.80 ms |
| 8 | `lidar_invalid`, `target_lidar_invalid` | `valid_fraction` ≥ 0.1 | yes (0.74) | yes (`valid=266` of 360) |
| 9 | `future_occupancy_ungrounded` | the target lidar carries `occupancy.cells` and `occupancy.observed` | yes | yes |
| 10 | `pose_confidence`, `target_pose_confidence` | `confidence` ≥ 0.1 | no pose stream | yes (ICP confidence 0.35–0.95) |
| 11 | `applied_action_missing`, `action_coverage` | intervals covering ≥ 0.8 of the pair's window, `authority == ACTION_AUTHORITY_LEASH` | **no applied-action stream** | yes: 2 997 intervals, coverage mean **1.0** |

The one rejected candidate of the 1 500 in this session is `{"action_coverage": 1}` — a window in
which the transport ledger's intervals did not cover the whole pair, which is the honest shape of a
ledger that stops when the transport does.

So the acceptance this ticket carries — "≥1 valid transition" — was not reachable by the calibration
producer alone: the same run would have moved from `calibration_missing` to `pose_missing` and stayed
at zero. Three producers were needed, and all three are real: the calibration producer (§2), the
existing pose runner (§3), and the transport ledger (§4).

## 2. The calibration producer

`qualia-calibration` observes the live rig for a bounded window and writes `qualia.calibration.v1`:

- from the leash's `observe`, the ranging device's own name, frame and angular sampling, and the camera
  surface the frames arrive on;
- from the arena's camera slot, the frame geometry the camera runner actually publishes and the rate it
  achieved;
- from the arena's lidar slot, the beam count and the published scan rate.

The run's own document (`board/calibration.json`, verbatim bar the elision):

```json
{
  "schema_version": "qualia.calibration.v1",
  "calibration_id": "cac618af21db53843124b949d06e78c41c7b55d91a6cfdf2862813a61760f6e2",
  "entity": "pinkie",
  "measured_at_ns": 1789195577494457568,
  "observation": {"window_ms": 8000, "leash_polls": 78, "leash_failures": 0, "camera_frames": 40, "lidar_scans": 78},
  "sensors": {
    "camera": {"surface": "/camera/stream.mjpg", "source_width": 640, "source_height": 480,
               "thumb_width": 64, "thumb_height": 48, "frames_observed": 40, "measured_rate_hz": 5.0026536,
               "luminance_mean_min": 0.38134062, "luminance_mean_max": 0.3839512,
               "intrinsics": {"status": "unmeasured", "why": "no target or known correspondence is observed, so no focal length or principal point is measurable here"},
               "extrinsics": {"status": "unmeasured", "why": "no camera/lidar correspondence is observed, so the transform between them is not measurable here"}},
    "lidar": {"source": "waveshare-ugv-ld06", "frame_id": "base_scan", "beams": 360,
              "angle_min_rad": -3.1415927, "angle_increment_rad": 0.017453292,
              "scans_observed": 78, "measured_rate_hz": 9.724553, "owner_rate_hz": 10.000793,
              "extrinsics": {"status": "unmeasured", "why": "the device's mounting transform is not observable from its own rotation alone"}}
  },
  "identity": {"identity_schema": "qualia.calibration-identity.v1", "entity": "pinkie",
               "camera": {"surface": "/camera/stream.mjpg", "source_width": 640, "source_height": 480, "thumb_width": 64, "thumb_height": 48},
               "lidar": {"source": "waveshare-ugv-ld06", "frame_id": "base_scan", "beams": 360, "angle_min_rad": -3.1415927, "angle_increment_rad": 0.017453292}}
}
```

`calibration_id` is a **sha256 over exactly the `identity` object**, carried in the document so the id
can be re-derived from the file. The producer's own log, from `board/logs/calibration.log`:

```text
qualia-calibration: observing http://127.0.0.1:8000/mcp observe and shm /qualia_body for 8000ms
qualia-calibration: observed camera frames=40 (640x480, thumb 64x48, luma 0.381..0.384, 5.003Hz) scans=78 (360 beams, 9.725Hz) leash_polls=78 failures=0
qualia-calibration: calibration_id=cac618af21db53843124b949d06e78c41c7b55d91a6cfdf2862813a61760f6e2 camera=640x480 lidar=waveshare-ugv-ld06 frame=base_scan beams=360 angle_min_rad=-3.1415927 angle_increment_rad=0.017453292
qualia-calibration: wrote /home/jetson/t65-capture/t65-real-01/calibration.json (unmeasured, named in the document: camera intrinsics, camera->lidar extrinsics, lidar mount transform)
```

That id is what the recorder stamps into every camera record, through its documented
`QUALIA_CALIBRATION_ID` (`runners/arena-recorder/src/main.rs:213`).

**What it refuses to invent.** Camera intrinsics, the camera↔lidar extrinsics and the ranging device's
mounting transform are recorded as `status: "unmeasured"` with the reason. Nothing in a window of live
traffic carries a target or a known correspondence, and a focal length nobody measured is the kind of
number that later gets quoted as a calibration. The dataset's gate is an identity check — the two
frames of a transition must come from one calibrated rig (`lib.rs:526-534`) — and that is what this
producer answers.

The measured **rates** are deliberately outside the digest: they drift run to run (this window measured
9.724553 Hz against the owner's own 10.000793 Hz), and including them would make two sessions of one
unchanged rig disagree on the identity they share.

## 3. Pose

`runners/pose` is unchanged by this ticket; it is only run. It registers each accepted scan against the
previous one with a small ICP solve and publishes a `NavPose` per registration, and it produced 2 917
pose records here — one per lidar scan, at the camera's own 150 ms gate with room to spare.

On a bench robot those rotations are of a **stationary scene**, which is a real measurement of a
stationary robot and not a simulation of motion, but it is worth reading what the estimator actually
published. From `board/logs/pose.head-tail.log`:

```text
qualia-pose: initialized pose from first scan points=132
qualia-pose: pose_seq=4 x_m=-0.001 z_m=-0.001 yaw_deg=0.13 score=0.012 matches=134 keyframes=1 edges=0 odom_edges=0 loop_edges=0
...
qualia-pose: pose_seq=6112 x_m=-1.375 z_m=-1.822 yaw_deg=18.22 score=0.008 matches=132 keyframes=12 edges=11 odom_edges=11 loop_edges=0
```

The pose walked to `(-1.375, -1.822)` m, `18.22°`, with a registration score of `0.008` m and 132
matched points: the graph's anchor chain accumulating small ICP displacements over 317 s on a scene
that did not move. That is the estimator's real output on real scans, and the number to read is the
score — sub-centimetre agreement between consecutive rotations — not the trajectory. `pose.confidence`
comes from the fit quality (`runners/pose/src/main.rs:829-834`) and stays in `[0.35, 0.95]`, above the
0.1 gate.

## 4. The transport ledger

The dataset accepts an applied action only when the record's `authority` is the transport's own
(`ACTION_AUTHORITY_LEASH`). On this robot the transport *is* the leash: it owns `/dev/ttyTHS1`, so
`runners/drive` cannot reach the wire and its records carry `ACTION_AUTHORITY_QUALIA_DRIVE`. For a
bench session the only real command is the one that keeps the robot stopped: the leash's own `stop`
tool, "Send a non-latching zero-speed motor stop" (`safety: physical-stop`, no token or approval in its
input schema). Probed once from the dev host before the board run, and quoted by the runner at startup
on the board:

```json
{"left":0.0,"max_speed":0.35,"ok":true,"right":0.0,"soft_odometry_limited":false,"speed_mode":"medium","stopped_by_deadman":false}
```

`runners/leash-transport` calls that tool ~10×/s and records each acknowledgement as one interval whose
fields are copied from the leash's reply: `valid` is the leash's `ok`, `clamped_*` and `applied_*` are
its `left`/`right`, `safety_flags` uses the leash's own vocabulary, and `armed` copies the harness's
`health`. From `board/logs/leash-transport.log`:

```text
qualia-leash-transport: leash health mode=live deadman_ok=true estop=false -> armed=true
qualia-leash-transport: leash acknowledged the stop: {"left":0.0,"max_speed":0.35,"ok":true,"right":0.0,"soft_odometry_limited":false,"speed_mode":"medium","stopped_by_deadman":false}
qualia-leash-transport: interval 1 …..884 ok=true left=0.000 right=0.000 max_speed=0.350 speed_mode=medium flags=0x0 authority=leash armed=true accepted=1 refused=0
qualia-leash-transport: interval 3100 …..008 ok=true left=0.000 right=0.000 max_speed=0.350 speed_mode=medium flags=0x0 authority=leash armed=true accepted=3100 refused=0
```

**No motion was commanded.** The command is a stop; every interval's speeds are `0.000`; `accepted=3100
refused=0` over the run; and `/qualia/action/applied` carries 2 997 of those intervals into the MCAP.
The ledger proves the *transport path* — the body's owner acknowledging a command with leash authority
— and nothing about locomotion. The robot was not driven, and `runners/drive` was not started: it
could not have opened `/dev/ttyTHS1` (leash holds it, `EBUSY`; D-025).

## 5. The session (board)

Commands, board-local 2026-09-12, from the lease comment on #250:

```text
$ ssh pinkie
$ ~/qualia-deploy/T65/docs/evidence/T65/calibration/t65-session.sh t65-real-01 300 8
CALIBRATION_RC=0
CALIBRATION_ID=cac618af21db53843124b949d06e78c41c7b55d91a6cfdf2862813a61760f6e2
RECORDER_RC=0
INSPECT_RC=0
```

The tree was built once, on the board, `RUSTFLAGS="-C target-feature=+fp16" cargo build --release -j 2`
for the union package set: **`Finished` in 19 m 52 s, `BUILD_RC=0`**, one compile, no ICE. The session
script starts `qualia-init`, `qualia-camera`, `qualia-leash-sensors`, `qualia-pose` and
`qualia-leash-transport`, runs `qualia-calibration` for an 8 s window, and only then starts
`qualia-arena-recorder` with the id it measured.

| Quantity | Value | Source |
| --- | --- | --- |
| MCAP | **30 300 801 B** | `board/mcap.size` |
| sha256 | **`03ae6b0f4d2e44970d4e47e155c50cd46c74e73b9ba847f6d4e8508b35287110`** | `board/mcap.sha256` |
| Window | `1789195562455027072` → `1789195879120836672` (300 s requested, 316 s start→end) | `board/window.txt` |
| `/qualia/camera` | 1 501 records (5.0 Hz) | `board/logs/inspect.log` |
| `/qualia/lidar` | 2 917 records (9.7 Hz) | `board/logs/inspect.log` |
| `/qualia/pose` | 2 917 records | `board/logs/inspect.log` |
| `/qualia/action/{applied,clamped,requested}` | 2 997 records each, `authority=leash` | `board/logs/inspect.log` |

The camera's log spans `frame_seq=2` … `3122` — 1 561 seqlock publishes (`+2` per publish,
`crates/types/src/lib.rs:413-436`), of which the recorder stored 1 501 — and its luminance is
0.381–0.384 with `quality=usable` throughout; the room was darker than during T58's capture
(0.437–0.471), which is a lighting fact, not a pipeline one, and both sit inside the gate's 0.05–0.95
band. The LD06 stream was the same device: `points=360 valid=266 rate=9.983Hz` at `scan 3050`
(`board/logs/leash-sensors.log`).

Nothing opened a device the leash owns. The camera was read through its MJPEG surface, the lidar
through `runners/leash-sensors` (the leash's `observe`), the transport through the leash's MCP `stop`,
and the pose from our own arena. The session's device check is in the capture log: `lsof` shows `leash
1299` holding `/dev/ttyACM0` and `/dev/ttyTHS1`, unchanged from D-025.

## 6. The dataset

```text
$ ~/qualia-deploy/T65/docs/evidence/T65/calibration/t65-dataset.sh t65-real-01 pinkie-bench bench-static pinkie
valid=1499 candidates=1500 sessions=1 environments=1 conditions=1
qualia-jepa-dataset: dataset does not meet the 50k/12-session/3-condition/3-environment gate
DATASET_RC=1
```

**1 499 of 1 500 candidate transitions are valid**, from one real 300 s session, and the audit
(`board/dataset-manifest.audit.json`, digest `6f98b125196ce48501410726309fa6c8b10b21ab4f1b67e0f61297851836b58d`):

```json
{"candidate_transitions":1500,"valid_transitions":1499,"rejected":{"action_coverage":1},
 "sessions":1,"environments":1,"conditions":{"bench-static":1499},"split_samples":{"train":1499},
 "mean_sensor_skew_ns":29192462.559039358,"p95_sensor_skew_ns":49796576,
 "mean_action_coverage":1.0,"overexposed_candidate_fraction":0.0}
```

The dataset binary still exits 1 — but at a **later named gate**, the dataset-level audit it exists to
enforce: one session cannot be 12, one environment cannot be 3, and 1 499 transitions cannot be 50 000.
That refusal is the promotion gate's, and it is the honest end of this ticket: the per-transition gates
all pass, the dataset-level ones are a question of how many sessions and places the robot is driven in.

## 7. The trainer's preflight

```text
$ qualia-jepa-train --manifest …6f98b125….json --checkpoint-id t65-real-e1 --output-dir train --epochs 1 --batch-size 32 --seed 65
qualia-jepa-train: dataset manifest is unsupported or has an invalid digest
TRAIN_RC=1
```

The trainer got past calibration — and past every other per-transition gate, as the dataset's own audit
shows — but it did **not** reach the promotion-gate message. `preflight`
(`crates/jepa-model/src/bin/qualia-jepa-train.rs:354-372`) checks the manifest's digest before calling
`validate_dataset_promotion_gate`, and the manifest the dataset just wrote does not reproduce its own
digest when the trainer recomputes it with the *same* function
(`qualia_jepa_dataset::manifest_digest`, SHA-256 over the manifest with `digest` cleared,
`crates/jepa-dataset/src/lib.rs:738-744`).

That is a finding, not a claim: the manifest is written and loaded by one crate, the digest is computed
by one function, and it matched on T58's manifest (whose audit numbers were all zero) and does not
match on this one (whose audit carries `mean_sensor_skew_ns: 29192462.559039358` and
`mean_action_coverage: 1.0`). The most likely mechanism is a serialization round-trip difference in the
audit's non-trivial floats, but this ticket did not chase it and does not claim it: what is measured is
that the trainer refuses at the manifest-integrity step, with the message above, before the promotion
gate. It is recorded here so the next ticket starts from the observation instead of rediscovering it.

## 8. What this does not establish

- **Not a promotion-passing dataset.** One session cannot satisfy the 50 k / 12-session / 3-condition /
  3-environment audit. The gate that stops it is named in §6.
- **Not intrinsics.** Unmeasured and named in the document (§2).
- **Not driving.** The transport ledger exercises a stop, at zero speed, on a stationary robot (§4).
- **Not a trajectory.** The pose stream is a stationary robot's ICP output and drifted as §3 shows.
- **Not every device question.** The camera's own USB identity (`0bda:5842`) and the LD06's
  (`1a86:55d3`) are the enumeration's facts, in D-025 and `docs/evidence/T58/real-sessions/README.md`
  §1; this producer identifies the rig by the surface and the owner's device name it can measure.

## 9. Reproduce

`t65-session.sh <session> <seconds> <calibration-window-seconds>` and
`t65-dataset.sh <session> <environment> <condition> <entity>` in this directory are the board's own
scripts. The two commands in §5 and §6 are what ran; the scripts' output, the raw logs, the manifest
audit, the digest and the size are in `board/`.
