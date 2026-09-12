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
