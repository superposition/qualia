# T58 real-sessions — the robot's own sensors, end to end

Step 58 of ticket [#239](https://github.com/superposition/qualia/issues/239). The point of this capture
is that **no number in it comes from a synthesized session**: the camera frames and the range scans are
the robot's, taken on Pinkie while the robot sat on the bench, and they are the input the dataset and
the trainer were pointed at. The synthetic gate-minimum catalog that fed T47, T50, T52 and T53 is
retired from this path and named in the last section.

## 1. The hardware, enumerated (board-local 2026-09-12 00:56–01:05)

Commands, in order, with their observed output:

```text
$ ls -l /dev/video*
crw-rw---- 1 root video 81, 0 Dec 31  1969 /dev/video0
crw-rw---- 1 root video 81, 1 Dec 31  1969 /dev/video1

$ ls -l /dev/ttyUSB* /dev/ttyACM*
ls: cannot access '/dev/ttyUSB*': No such file or directory
crw-rw---- 1 root dialout 166, 0 Dec 31  1969 /dev/ttyACM0

$ ls -l /dev/iio:device*
ls: cannot access '/dev/iio:device*': No such file or directory

$ v4l2-ctl --list-devices
NVIDIA Tegra Video Input Device (platform:tegra-camrtc-ca):
	/dev/media0
USB Camera: USB Camera (usb-3610000.usb-2.2):
	/dev/video0
	/dev/video1
	/dev/media1

$ lsusb
Bus 001 Device 005: ID 0bda:5842 Realtek Semiconductor Corp. USB Camera
Bus 001 Device 007: ID 03e7:2485 Intel Movidius MyriadX
Bus 001 Device 008: ID 1a86:55d3 QinHeng Electronics USB Single Serial
Bus 001 Device 006: ID 0c76:1229 JMTek, LLC. USB PnP Audio Device
Bus 001 Device 003: ID 0bda:c822 Realtek Semiconductor Corp. Bluetooth Radio

$ timeout 5 tegrastats
09-12-2026 00:56:25 RAM 1487/3602MB (lfb 8x4MB) SWAP 435/14089MB (cached 6MB) CPU [3%@1510,1%@1510,1%@729,3%@729,3%@729,3…
```

The device nodes do not say who owns them. The owner is the leash:

```text
$ lsof /dev/ttyACM0 /dev/ttyTHS1          # via sudo
COMMAND  PID   USER   FD   TYPE DEVICE SIZE/OFF NODE NAME
leash   1299 jetson   45uW  CHR 166,0      0t0  833 /dev/ttyACM0
leash   1299 jetson   44uW  CHR 240,1      0t0  153 /dev/ttyTHS1

$ grep -E 'SERIAL_PORT|LIDAR_DEVICE|CAMERA_DEVICE|PROFILE|LISTEN' ~/.config/leash/leash.env
LEASH_PROFILE=waveshare-ugv
LEASH_LISTEN=0.0.0.0:8000
LEASH_SERIAL_PORT=/dev/ttyTHS1
LEASH_SERIAL_BAUD=115200
LEASH_CAMERA_DEVICE=/dev/video0
LEASH_UGV_LIDAR_DEVICE=/dev/ttyACM0
```

`stty -F /dev/ttyACM0` and `head -c 120 /dev/ttyACM0` both fail with `Device or resource busy`, so the
LiDAR's port is not ours to open: it is a single-owner device and its owner republishes it.

**The I2C question, closed.** No IIO device exists (`/sys/bus/iio/devices` is empty), so the inertial
unit was looked for on the buses the Orin exposes:

```text
$ i2cdetect -y -r 1        # and -r 0, -r 7
     0  1  2  3  4  5  6  7  8  9  a  b  c  d  e  f
20: -- -- -- -- -- UU -- -- -- -- -- -- -- -- -- --
40: UU -- -- -- -- -- -- -- -- -- -- -- -- -- -- --

1-0025 -> fusb301    (USB-C controller)
1-0040 -> ina3221    (power monitor)
0-0050, 0-0057 -> 24c02   (EEPROMs)
4-003c -> vrs-pseq   (power sequencer)
i2c-7: 0x15, 0x3c, 0x42
```

No `0x68`, `0x69`, `0x76`, `0x28`, `0x6a` or `0x6b` answers anywhere: **there is no IMU on I2C on this
board**, and none on IIO. The robot's inertial data is the leash's.

### What is live, and where

| Sensor | State | Where it is readable |
| --- | --- | --- |
| Camera (UVC MJPG 1920×1080@30, `0bda:5842`) | live | `/dev/video0` **owner: leash** (`LEASH_CAMERA_DEVICE`) → `http://127.0.0.1:8000/camera/stream.mjpg` |
| LD06 LiDAR (`1a86:55d3`, `/dev/ttyACM0`) | live | leash's `sensors.range_scan`, `source: waveshare-ugv-ld06`, 9.9958 Hz, 360 ranges + intensities |
| IMU (9-DOF incl. magnetometer) | live | leash's `sensors.imu` + `sensors.raw_frame.payload.{ax,ay,az,gx,gy,gz,mx,my,mz}` |
| Wheel odometry + battery | live | leash's `sensors.odometry`, `sensors.battery` (83.6 %, 12.01 V) |
| Drive serial `/dev/ttyTHS1` | live | **owner: leash** (`LEASH_SERIAL_PORT`) |
| Localization | **absent** | leash `localization.health.status: "unavailable"`, `pose: null`, provider `initializing` |
| Intel Movidius MyriadX (`03e7:2485`) | enumerated, no driver | no `/dev/video*` node, nothing bound |
| Second camera node `/dev/video1` | same UVC device | second node of the one camera; `/dev/media0` (Tegra VI) has no node of its own |

The leash's own sensor surface, quoted from a live `observe` call (`leash-observe.json` in this
directory):

```json
"sensors": {
  "camera": {"health": "healthy", "snapshot_url": "/camera/snapshot", "stream_url": "/camera/stream.mjpg", "status": "available"},
  "imu": {"last_ms": 1789189308233, "sample": {"angular_velocity_radps": {"x": -0.00286, "y": 0.00429, "z": 0.00143},
           "linear_acceleration_mps2": {"x": -0.141, "y": 0.0263, "z": 9.792}, "frame_id": "base_link"},
          "source": "waveshare-ugv", "status": "available"},
  "range_scan": {"last_ms": 1789189308297, "sample": {"angle_increment_rad": 0.017453292519943295,
                 "angle_max_rad": 3.12413936106985, "angle_min_rad": -3.141592653589793,
                 "frame_id": "base_scan", "intensities": [159.0, 152.0, ...], "ranges_m": [1.711, 1.722, ...],
                 "scan_rate_hz": 9.99583333333334, "ts_ms": 1789189308297},
                 "source": "waveshare-ugv-ld06", "status": "available"}
}
```

## 2. How the capture reads it

Neither device is opened. The camera is read by `qualia-camera`'s existing MJPEG path
(`QUALIA_CAMERA_STREAM_URL`), and the range scan by `runners/leash-sensors`, which polls the leash's
`observe` tool and publishes the rotation through `qualia-lidar`'s own `publish_scan` (so the polar
scan and the occupancy grid keep one implementation, `runners/lidar/src/lib.rs:314`).
`qualia-arena-recorder` then writes the arena into MCAP exactly as it always did. The decision and its
measurements are recorded as D-023 in `docs/decisions.md`.

Measured on the MJPEG stream from the board and from this workstation:

```text
$ curl -s -D - -o /tmp/t58-mjpg.bin http://127.0.0.1:8000/camera/stream.mjpg   # on the board
HTTP/1.1 200 OK
content-type: multipart/x-mixed-replace; boundary=leashframe
991232 bytes, 19 JPEG SOI markers in 5 s

$ curl -s -D - -o t58-host-mjpg.bin http://192.168.55.1:8000/camera/stream.mjpg  # from the workstation
1794048 bytes, 31 JPEG SOI markers in 6 s
```

## 3. What could not be recorded, and why

- **LiDAR/odometry as MCAP topics** — see the capture table below: this is what the capture does
  record where the surfaces are live.
- **IMU** — the arena has no inertial slot and MCAP has no IMU topic (`crates/mcap-log`: the twelve
  topics are action/{requested,applied,clamped}, belief, camera, health, jepa, lidar, planner, pose,
  prior, vslam). Adding one is a wire-interface change, not a capture setting; the inertial stream
  stays on the leash until a ticket owns that interface.
- **Applied actions** — `TOPIC_ACTION_APPLIED` is drained from the arena's applied-action history,
  which is written by whoever transports a command to the physical layer. On this robot that is the
  leash (it owns `/dev/ttyTHS1`), so `runners/drive` cannot run and no `transport_accepted` interval
  exists. The bench robot was not driven.
- **Pose** — the arena's canonical pose carries a confidence claim the dataset gates on. The leash's
  localization provider reports `pose: null` and its wheel odometry carries only a covariance, so
  publishing a confidence would be inventing the number the gate reads. It is left unwritten and
  named here instead.

## 4. The synthetic path, retired

Every generator, scratch recipe and synthesize helper that has ever fed this pipeline, where it lives,
and what happens to it:

| Recipe | Where it lives | What it fed | Disposition |
| --- | --- | --- | --- |
| The **T52 fixture writer** — a scratch crate that writes sealed MCAP sessions (`wave A`, `ch16`, `ch4`) plus their catalogs; its `Cargo.toml` and sources are quoted verbatim in `docs/evidence/T52/checkpoint-experiments/README.md` (~lines 328–370) | **not in this tree**; host scratch (`/mnt/c/tmp/Impl52…`, `/mnt/c/tmp/Impl53…`) | T47's training-step capture, T50's row D, T52's checkpoint experiments and T53's encoder-gradient measurement — the 12-session × 4168-frame gate-minimum catalog | **retired from every evidence path.** It stays quoted as the T52 capture's record of how its input was made, and the captures keep their numbers; no ticket after T58 may point the dataset or trainer at its output, because this capture demonstrates the real path instead |
| T53's reuse of that ch4 manifest (`dataset_digest 26e2dfac…`, 12 sessions, 42 000/4 200/4 200) | `docs/evidence/T53/encoder-gradients/runs/` | T53's before/after encoder measurement | **historical record only**; superseded as evidence by this capture |
| `sealed_session()` / `camera_only_session()` — write sealed MCAPs from synthetic frames | `crates/jepa-dataset/src/tests.rs`, behind `#[cfg(test)] mod tests;` (`crates/jepa-dataset/src/lib.rs:1583`) | `qualia-jepa-dataset`'s unit tests | **stays**: a unit-test fixture, compiled out of every shipped binary |
| `sealed_catalog_entry()` and the 12-session catalog builder | `crates/jepa-dataset/tests/cli.rs` | the dataset binary's integration test | **stays**: test-only |
| `build_fixture()` — the frozen synthetic runtime input | `crates/jepa-model/src/parity.rs:303`, **not** under `#[cfg(test)]` | `qualia-jepa-parity`'s no-checkpoint mode, which compares **backends** on a frozen input (its report is tagged `qualia.jepa-parity-fixture.v1` with a `fixture_sha256`) | **stays, named**: it feeds no dataset and no quality claim — it launches the real kernels on both backends and measures their agreement. Retiring it would delete the CUDA lane's parity check and change a shipped binary's interface |
| `synthetic_batch()` | `crates/jepa-model/tests/contract.rs` | the trainer's step test | **stays**: test-only |
| Registry report/manifest builders | `crates/jepa-registry/src/lib.rs:959`, behind `#[cfg(test)]` | the registry's tests | **stays**: test-only |
| `offline_response` / the offline world model | `runners/vision` (documented in its module header as the offline branch that "builds a synthetic scene from the sensed belief") | the vision runner's offline mode | **stays, named**: it is a runner mode, not a JEPA dataset/training input, and no ticket's JEPA evidence reads it |

The statement this ticket lands: **no evidence path reads a generator.** The dataset's only input is
sealed MCAP written by `qualia-arena-recorder` from the arena, the trainer's only input is that
manifest, and for this capture both point at the sessions in section 3. The four synthetic MCAP
writers that do exist are compiled out of every shipped binary (`#[cfg(test)]` or `tests/`), and the
one synthesizer that is not — `build_fixture()` — produces a backend-parity input, not a session.

## 5. The capture, the dataset, the trainer

The bounded capture ran on 2026-09-12, board-local **01:37:51 → 01:52:51 EDT** (epoch
1789191471964593088 → 1789192381565040000, elapsed **909 s**), bounded by
`QUALIA_MCAP_DURATION_SECONDS=900`. The exact scripts are `t58-capture.sh` and `t58-dataset.sh` in
this directory; `capture-01.log` and `dataset-01.log` are their unelided output.

| Session | Sensors taken | Window (board-local) | MCAP | Size | sha256 |
| --- | --- | --- | --- | --- | --- |
| `t58-real-smoke` | camera + LD06 lidar | 01:36:28 → 01:37:28 (60 s) | `t58-real-smoke.mcap` | 5 559 657 B | `10a228f60923644adae784f12215bb9a15f1038f45e28bde305966e133665e96` |
| `t58-real-01` | camera + LD06 lidar | 01:37:51 → 01:52:51 (900 s) | `t58-real-01.mcap` | **83 299 459 B** | `97612acb339716e2e94e03157b29c5419a706c42f3c37d52b880ba3d20c13334` |

Both files stay on the board at `~/t58-capture/<session>/mcap/` — they are evidence, not repository
content; the sizes and digests above are what a later agent checks them by.

```text
$ qualia-mcap-inspect ~/t58-capture/t58-real-01/mcap/t58-real-01.mcap
INSPECT_RC=0
/qualia/camera           pinkie   messages=  4502 span_ns=  900076073472 payload_bytes=52452153
/qualia/lidar            pinkie   messages=  8775 span_ns=  899988080288 payload_bytes=4846499653
```

4 502 camera records over 900.08 s (5.0 Hz, the leash's configured framerate) and 8 775 LiDAR records
over 899.99 s (9.75 Hz, the LD06's own rotation rate). Every record is the robot's: the camera
thumbnails decode at 640×480 with `luma_mean` 0.464–0.468 and `stddev` 0.150 (`quality=usable`) the
whole way through, and each LiDAR record carries 360 points of which 265–269 carry a return
(`qualia-leash-sensors: scan 8800 … points=360 valid=268 rate=9.996Hz`). `/qualia/pose`,
`/qualia/action/*`, `/qualia/belief` and `/qualia/health` are silent channels: nothing publishes
them on this robot (section 3).

### The dataset built from it

```text
$ qualia-jepa-dataset --catalog catalog.json --output-dir dataset
/home/jetson/t58-capture/t58-real-01/dataset/jepa-dataset-fb309e6f160badff5d5493f09ba194cb50727c48e512e4744fd8cc477f8d5a75.json
valid=0 candidates=4501 sessions=0 environments=0 conditions=0
qualia-jepa-dataset: every promoted MCAP source must contribute valid transitions
DATASET_RC=1
```

The manifest it wrote before refusing, quoted from `dataset-manifest.json`:

```text
schema qualia.jepa-dataset.v3
digest fb309e6f160badff5d5493f09ba194cb50727c48e512e4744fd8cc477f8d5a75
sources 1 samples 0
valid_transitions 0 candidate_transitions 4501
sessions 0 environments 0 conditions {}
rejected {"calibration_missing": 4501}
split_samples {}
mean_sensor_skew_ns 0.0 p95_sensor_skew_ns 0
mean_action_coverage 0.0 overexposed_candidate_fraction 0.0
```

**4 501 real transitions were formed and all 4 501 were rejected, every one for the same reason.**
The dataset walks camera pairs in order, so the first gate that fires decides the ledger, and the
first one is `calibration_missing` — the recorder's `calibration_id` is `unavailable` (its documented
default when `QUALIA_CALIBRATION_ID` is unset, and the truthful value here: this repository has no
calibration producer, only the registry's test fixture carries a `calibration_id`). So the real
capture reaches the dataset's first physical gate with 4 501 candidates and stops there; the streams
that are missing *after* calibration — pose and applied actions — are not even reached.

### The trainer

```text
$ qualia-jepa-train --manifest …fb309e6f….json --checkpoint-id t58-real-e1 --output-dir train \
      --backend cuda --epochs 1 --batch-size 32 --seed 58
qualia-jepa-train: every promoted MCAP source must contribute valid transitions
TRAIN_RC=1
```

The refusal is `preflight`'s (`crates/jepa-model/src/bin/qualia-jepa-train.rs:355-372`), which runs
`validate_dataset_promotion_gate` **before** `device_for_backend`, so no backend was selected and no
epoch ran. That is the honest end of the training leg today: **there is no real checkpoint to
measure**, and the gate metrics T53 published cannot be recomputed on this data because the data
never reaches the model.

### Against the synthetic baseline (T53's ch4-e1, one epoch, CUDA)

| Quantity | T53 synthetic (`26e2dfac…`, 12 sessions × 4 200) | T58 real (`fb309e6f…`, 1 session) |
| --- | --- | --- |
| valid transitions | 50 400 | **0** of 4 501 candidates |
| sessions / environments / conditions | 12 / 3 / 3 | 0 / 0 / 0 offered as 1 / 1 / 1 |
| calibration slope (val / test) | 0.531798 / 1.15682 (band 0.9–1.1) | not computable |
| `mean_std_resid` (val / test) | 1.19997 / 1.15455 (band 0.9–1.1) | not computable |
| rollout error (val / test) | 0.0150985 / 0.0410029 | not computable |
| `effective_rank` (floor 64) | **1.20479** | not computable |
| baseline gate / grounding+calibration gate | true / false | refused before evaluation |

The synthetic column is quoted from the committed `docs/evidence/T53/encoder-gradients/runs/ch4-e1/`
report; the real column is empty for one reason — the dataset gate refuses the real capture before
training starts — and that emptiness is the measurement.

## 6. Handoff to T52 (#225)

**No promotion-passing checkpoint came out of this ticket, and the reason is now a short, ordered
list rather than "more data".** In the order the pipeline asks for them:

1. **A calibration artifact.** This is the first gate and the only one the real capture reaches: the
   repository has no producer of a `calibration_id` (grep `calibration_id` — a struct field, the
   recorder's env default, and one registry test fixture; nothing computes one), so a real session
   can never admit a transition. This is the cheapest fix of the four and it is a *code* fix, not a
   data fix, and it is what T52 needs before any of the rest matters.
2. **A pose stream.** `TOPIC_POSE` needs the arena's canonical pose with a confidence claim. The
   leash owns the drive and its localization provider reports `pose: null` with the provider
   `initializing`, so today the only candidate source is wheel odometry, which carries a covariance
   and no confidence. Either leash localization comes up or `runners/pose` runs against the leash's
   lidar and camera.
3. **Applied actions.** `TOPIC_ACTION_APPLIED` requires a `transport_accepted` interval, written by
   whoever drives the physical layer — leash again, and its port is not ours. A bench capture of a
   robot nobody drives contributes none, so transitions would still be rejected at
   `applied_action_missing` even with 1 and 2 fixed.
4. **The scale and variety the gate names.** 50 000 valid transitions, 12 sessions, 3 environments,
   3 conditions, ≥4 096 per held-out split. One 15-minute bench session produced 4 501 candidates
   (≈5/s), so the *magnitude* is reachable — ~3 hours of recording for 50 k — but 3 environments and
   3 conditions are a physical fact about where the robot is driven, not something a longer recording
   produces.

What this ticket therefore hands T52 is not a checkpoint but a decision it can act on: the milestone
that unblocks promotion is **a calibration path plus a driven, localized session set**, and no amount
of synthetic sessions substitutes for either (T52/T53 already measured that: four fixtures and a 10×
epoch run left `effective_rank` at 1.1–1.45 against the floor of 64).


