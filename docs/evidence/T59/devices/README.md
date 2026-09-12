# T59 — every runner on the real device

Ticket [#240](https://github.com/superposition/qualia/issues/240). This directory is the audit the
ticket asks for: what the robot's hardware is as Pinkie presents it, one line per runner with the
command that was run and what it printed, and what is genuinely absent. It is not a profiler
capture — it commits no `capture.json`, no `kernels.json` and no `capture.sqlite`
([`docs/evidence/README.md`](../../README.md) §Layout).

Two rules are read through every row:

* **D-025 (drafted, and cited in the 01:06 comment, as D-023) — the leash owns the hardware.** On
  Pinkie one process, `leash serve http` (pid 1299), holds the LD06's serial port, the drive port
  and the UVC node. The stack subscribes to its surface; it does not open devices.
  `runners/leash-sensors` (the MCP `observe` bridge) and `runners/camera` (the MJPEG consumer) are
  the two runners that do the subscribing.
* **The region is the interface.** Every runner that is not a device reader consumes or produces
  the shared arena (`/qualia_body` by default); that is what "runs on the robot" means for it.

## The machine

Measured on Pinkie (`ssh pinkie`) in two windows, board-local 02:09–02:19 and 02:56–03:02 EDT
2026-09-12, with the board otherwise idle (before the first probe batch: `uptime` load 0.94; `ps -eo
pcpu,comm --sort=-pcpu | head` showed `leash` 4.5 % and `sshd` 3.0 % as the only entries above
0.5 %; before the second: load 0.65, no `cargo`/`rustc`, the 02:56 `date/uptime/ps/pgrep` quoted in
the lease on [#240](https://github.com/superposition/qualia/issues/240)). The first window is the
enumeration and probe batch; the second is the runner-level `qualia-lidar`/`qualia-drive` pass and the
full `i2cdetect` scan.

### What enumerates

```console
$ ls -l /dev/video* /dev/ttyUSB* /dev/ttyACM* /dev/ttyTHS* /dev/iio:device*
ls: cannot access '/dev/ttyUSB*': No such file or directory
ls: cannot access '/dev/iio:device*': No such file or directory
crw-rw---- 1 root dialout 166, 0 Dec 31  1969 /dev/ttyACM0
lrwxrwxrwx 1 root root         7 Aug 26  2025 /dev/ttyTHS0 -> ttyTHS1
crw-rw---- 1 root dialout 240, 1 Sep 12 02:13 /dev/ttyTHS1
crw-rw---- 1 root dialout 240, 2 Aug 26  2025 /dev/ttyTHS2
crw-rw---- 1 root video    81, 0 Dec 31  1969 /dev/video0
crw-rw---- 1 root video    81, 1 Dec 31  1969 /dev/video1
```

```console
$ lsusb
Bus 002 Device 002: ID 0bda:0489 Realtek Semiconductor Corp. 4-Port USB 3.0 Hub
Bus 001 Device 003: ID 0bda:c822 Realtek Semiconductor Corp. Bluetooth Radio
Bus 001 Device 007: ID 03e7:2485 Intel Movidius MyriadX
Bus 001 Device 005: ID 0bda:5842 Realtek Semiconductor Corp. USB Camera
Bus 001 Device 008: ID 1a86:55d3 QinHeng Electronics USB Single Serial
Bus 001 Device 006: ID 0c76:1229 JMTek, LLC. USB PnP Audio Device
Bus 001 Device 004: ID 1a40:0101 Terminus Technology Inc. Hub
Bus 001 Device 002: ID 0bda:5489 Realtek Semiconductor Corp. 4-Port USB 2.0 Hub
Bus 001 Device 001: ID 1d6b:0002 Linux Foundation 2.0 root hub
```

```console
$ lsusb -t
/:  Bus 01.Port 1: Dev 1, Class=root_hub, Driver=tegra-xusb/4p, 480M
    |__ Port 2: Dev 2, If 0, Class=Hub, Driver=hub/4p, 480M
        |__ Port 1: Dev 4, If 0, Class=Hub, Driver=hub/4p, 480M
            |__ Port 1: Dev 6, If 0-3, Class=Audio/HID, Driver=snd-usb-audio, usbhid, 12M
            |__ Port 2: Dev 8, If 0, Class=Communications, Driver=cdc_acm, 12M
            |__ Port 2: Dev 8, If 1, Class=CDC Data, Driver=cdc_acm, 12M
        |__ Port 2: Dev 5, If 0/1, Class=Video, Driver=uvcvideo, 480M
        |__ Port 2: Dev 5, If 2/3, Class=Audio, Driver=snd-usb-audio, 480M
        |__ Port 3: Dev 7, If 0, Class=Vendor Specific Class, Driver=, 480M
    |__ Port 3: Dev 3, If 0/1, Class=Wireless, Driver=btusb, 12M
```

```console
$ udevadm info -q path -n /dev/ttyACM0
/devices/platform/bus@0/3610000.usb/usb1/1-2/1-2.1/1-2.1.2/1-2.1.2:1.0/tty/ttyACM0
```

```console
$ ls /sys/bus/iio/devices
$ ls /dev/media*
crw-rw---- 1 root video 505, 0 Dec 31  1969 /dev/media0
crw-rw---- 1 root video 505, 1 Dec 31  1969 /dev/media1
```

`/dev/ttyACM0` is the `1a86:55d3` CDC device at USB path `1-2.1.2` — the LD06's USB bridge — and not
a CH340 on `ttyUSB`. There is no `/dev/ttyUSB*` node on this machine at all, which is why the
ticket's original `runners/lidar` default of `/dev/ttyUSB1` could never have opened anything here.

### Who holds what

```console
$ lsof /dev/ttyACM0 /dev/ttyTHS1 /dev/video0
COMMAND  PID   USER  FD   TYPE DEVICE SIZE/OFF NODE NAME
leash   1299 jetson  mem    CHR   81,0           719 /dev/video0
leash   1299 jetson  44uW  CHR  240,1      0t0  153 /dev/ttyTHS1
leash   1299 jetson  45uW  CHR  166,0      0t0  833 /dev/ttyACM0
leash   1299 jetson  48u   CHR   81,0      0t0  719 /dev/video0
```

```console
$ tr '\0' '\n' < /proc/1299/environ | grep -E '^LEASH' | sort
LEASH_ALLOW_PHYSICAL_ACTUATION=true
LEASH_CAMERA_BACKEND=v4l2
LEASH_CAMERA_DEVICE=/dev/video0
LEASH_CAMERA_FRAMERATE=5
LEASH_CAMERA_INPUT_FORMAT=mjpeg
LEASH_CAMERA_STREAM_CODEC=copy
LEASH_CAMERA_VIDEO_SIZE=640x480
LEASH_DEADMAN_MS=400
LEASH_DRIVE_INVERT=true
LEASH_LISTEN=0.0.0.0:8000
LEASH_POLICY_MODE=require-token
LEASH_PROFILE=waveshare-ugv
LEASH_ROLE=pinkie
LEASH_SERIAL_BAUD=115200
LEASH_SERIAL_PORT=/dev/ttyTHS1
LEASH_UGV_LIDAR_BODY_MASKS_DEG=45:136
LEASH_UGV_LIDAR_DEVICE=/dev/ttyACM0
```

A second opener of either serial port is refused; a second opener of the camera node is not:

```console
$ python3 -c "import os
for p in ('/dev/ttyACM0','/dev/ttyTHS1','/dev/video0','/dev/video1'):
    try:
        fd = os.open(p, os.O_RDWR | os.O_NONBLOCK); print(p, 'OPENED fd=%d' % fd); os.close(fd)
    except OSError as e:
        print(p, type(e).__name__, 'errno=%d' % e.errno, e.strerror)"
/dev/ttyACM0 OSError errno=16 Device or resource busy
/dev/ttyTHS1 OSError errno=16 Device or resource busy
/dev/video0 OPENED fd=3
/dev/video1 OPENED fd=3
```

**This corrects the record on this ticket.** The 01:06 comment and the decision then numbered D-023,
now **D-025**, said the leash holds `/dev/video0` "with `EBUSY` for anyone else". It does not: the
exclusivity is the two serial ports' (`uW` fds), while the UVC node takes several readers (`lsof`
shows fd `48u` plus an mmap, and the open above still returned a descriptor). The rule that the stack
subscribes rather than opens is unchanged — the camera's pixels reach the arena through
`runners/camera` for one owner and one implementation — but "the camera is EBUSY" must stop being
quoted as its reason.

### No IMU on this machine's own buses

```console
$ for b in 0 1 2 7; do echo "--- bus $b"; echo jetson | sudo -S i2cdetect -y -r $b; done
--- bus 0
[sudo] password for jetson:      0  1  2  3  4  5  6  7  8  9  a  b  c  d  e  f
00:                         -- -- -- -- -- -- -- -- 
10: -- -- -- -- -- -- -- -- -- -- -- -- -- -- -- -- 
20: -- -- -- -- -- -- -- -- -- -- -- -- -- -- -- -- 
30: -- -- -- -- -- -- -- -- -- -- -- -- -- -- -- -- 
40: -- -- -- -- -- -- -- -- -- -- -- -- -- -- -- -- 
50: UU -- -- -- -- -- -- UU -- -- -- -- -- -- -- -- 
60: -- -- -- -- -- -- -- -- -- -- -- -- -- -- -- -- 
70: -- -- -- -- -- -- -- --                         
--- bus 1
     0  1  2  3  4  5  6  7  8  9  a  b  c  d  e  f
00:                         -- -- -- -- -- -- -- -- 
10: -- -- -- -- -- -- -- -- -- -- -- -- -- -- -- -- 
20: -- -- -- -- -- UU -- -- -- -- -- -- -- -- -- -- 
30: -- -- -- -- -- -- -- -- -- -- -- -- -- -- -- -- 
40: UU -- -- -- -- -- -- -- -- -- -- -- -- -- -- -- 
50: -- -- -- -- -- -- -- -- -- -- -- -- -- -- -- -- 
60: -- -- -- -- -- -- -- -- -- -- -- -- -- -- -- -- 
70: -- -- -- -- -- -- -- --                         
--- bus 2
     0  1  2  3  4  5  6  7  8  9  a  b  c  d  e  f
00:                         -- -- -- -- -- -- -- -- 
10: -- -- -- -- -- -- -- -- -- -- -- -- -- -- -- -- 
20: -- -- -- -- -- -- -- -- -- -- -- -- -- -- -- -- 
30: -- -- -- -- -- -- -- -- -- -- -- -- -- -- -- -- 
40: -- -- -- -- -- -- -- -- -- -- -- -- -- -- -- -- 
50: -- -- -- -- -- -- -- -- -- -- -- -- -- -- -- -- 
60: -- -- -- -- -- -- -- -- -- -- -- -- -- -- -- -- 
70: -- -- -- -- -- -- -- --                         
--- bus 7
     0  1  2  3  4  5  6  7  8  9  a  b  c  d  e  f
00:                         -- -- -- -- -- -- -- -- 
10: -- -- -- -- -- -- -- -- -- -- -- -- -- -- -- -- 
20: -- -- -- -- -- -- -- -- -- -- -- -- -- -- -- -- 
30: -- -- -- -- -- -- -- -- -- -- -- -- 3c -- -- -- 
40: -- -- 42 -- -- -- -- -- -- -- -- -- -- -- -- -- 
50: -- -- -- -- -- -- -- -- -- -- -- -- -- -- -- -- 
60: -- -- -- -- -- -- -- -- -- -- -- -- -- -- -- -- 
70: -- -- -- -- -- -- -- --                         
$ ls /sys/bus/iio/devices
$ ls /dev/iio:device*
ls: cannot access '/dev/iio:device*': No such file or directory
```

**The scan is complete, and the block is the board's output pasted rather than transcribed**: every
bus's `00`–`70` grid is inside it, so the rows the IMU claim rests on are visible rather than asserted
— rows `60`/`70` (`0x68`, `0x69`, `0x6a`, `0x6b`, `0x76`) and `0x28` on row `20` are `--` on all four
buses. The block is the captured session byte-for-byte (`C:/tmp/Impl254T59Fixes/i2c-scan.txt`,
trailing spaces included); `--- bus N` is the loop's own `echo`, and the bus 0 header shares its line
with `sudo`'s prompt because that is how it printed. The excerpt this replaces showed only the
responder rows and added its own separators, and the block written from it dropped bus 2 and then
left bus 7's row `30` short of its 16 cells, with `3c` under column `0x3b`. Both faults are gone in a
way the block itself shows: bus 2's grid is present, and row `30` has all 16 cells with `3c` under
`0x3c`. The only responders are the ones D-025 already names: `0x25` and `0x40` on bus 1 (`fusb301`,
`ina3221`), `0x50`/`0x57` on bus 0 (the `24c02` EEPROMs, `UU` = kernel-held) and `0x3c`/`0x42` on bus
7, none of which is an inertial address.

**One line of D-025 does not survive the complete scan.** D-025 records `i2c-7`'s responders as
`0x15`/`0x3c`/`0x42`; the scan above shows `0x3c` and `0x42` on bus 7 and no `0x15` on any of the four
buses. Either that line is wrong or the `0x15` responder was absent at scan time; the decision entry
is reconciled on `main`, not in this audit.

No responder at `0x68`, `0x69`, `0x76`, `0x28`, `0x6a` or `0x6b`, and no IIO device: the robot's
inertial data exists **only** in the leash's `sensors.imu`/`raw_frame`. `runners/leash-sensors` reads
and reports the stream's availability (`leash surface imu=available(waveshare-ugv)`) and deliberately
writes nothing into the arena — there is no inertial slot and no `Imu` type in `crates/types`, so no
runner consumes it. The audit records that as a gap, not as a missing device.

### The rest of the machine

```console
$ echo jetson | sudo -S timeout 4 tegrastats --interval 1000
09-12-2026 02:14:33 RAM 1484/3602MB (lfb 21x4MB) SWAP 436/14089MB (cached 7MB)
  CPU [3%@729,2%@729,1%@729,0%@729,1%@729,1%@729] EMC_FREQ 0%@2133 GR3D_FREQ 0%@[305]
  NVDEC off NVJPG off NVJPG1 off VIC off OFA off APE 200 cpu@56.281C soc2@55.843C soc0@54.093C
  gpu@55.843C tj@56.281C soc1@56.187C VDD_IN 4428mW/4428mW VDD_CPU_GPU_CV 562mW/562mW
```

The `03e7:2485` MyriadX on `1-2.3` is the one accelerator-shaped USB device on the bus and it has no
driver bound and no `/dev/video*` node of its own: on this image it is inert. The audio device
(`0c76:1229`) and the Bluetooth radio are not robot hardware.

## Per runner

`device` = its normal path opens a real device or socket; `fixture-only` = it can only be exercised
with generated input today. The live runs below ran on the dev host against the real robot: an
`init`-owned private arena `/qualia_t59` whose producers are the two subscription runners pointed at
the board's leash (`QUALIA_LEASH_BASE_URL=http://192.168.55.1:8000`, camera stream
`http://192.168.55.1:8000/camera/stream.mjpg`), at `6ed5b61` + the working tree. That is the
operator's own topology (the stack on this host, the robot's leash on Pinkie) on a private region, so
the operator's live `/qualia_body` stack was untouched.

Two of the rows below are measured **on the board itself**, which is what the ticket's acceptance
asks for and what the first version of this audit left open: `qualia-lidar` and `qualia-drive` ran on
Pinkie at `99f56a5`, 2026-09-12 03:02 board-local, under the D-024 lease posted on
[#240](https://github.com/superposition/qualia/issues/240). Each was started with only
`QUALIA_SHM_NAME=/qualia_t59` set — the port, baud, tick and arming keys are the runners' own
defaults — against an `init`-owned private arena (`QUALIA_INIT_OWNER_ONLY=1`: `Owner-only mode:
holding shared memory without spawning runners`, region `/qualia_t59` and `/qualia_t59_stats`
created). Both refuse the port the leash holds and exit **2**, the documented "cannot open the port".
The success path (assembling rotations, driving the motors) is deliberately not reachable on this
robot: the leash owns both serial ports (D-025), so these rows measure the refusal and the exit code,
not the read loop.

```console
$ # Pinkie, 99f56a5, under the lease on #240; 02:56–03:02 board-local
$ export PATH=$HOME/.cargo/bin:$PATH CARGO_TARGET_DIR=$HOME/t59/target; cd ~/t59/src
$ cargo build --release -j 2 -p qualia-init -p qualia-lidar -p qualia-drive
    Finished `release` profile [optimized] target(s) in 3m 21s            # BUILD_RC=0, 03:00:09
$ cd ~/t59 && QUALIA_SHM_NAME=/qualia_t59 QUALIA_LOG_DIR=$HOME/t59/window \
      QUALIA_INIT_OWNER_ONLY=1 QUALIA_STACK_MANIFEST=$HOME/t59/manifest-t59.json \
      nohup $HOME/t59/target/release/qualia-init > window/holder.log 2>&1 &
[init] Creating shared memory '/qualia_t59'...
[init] Shared memory created: 64 MB
[init] Stats region created: '/qualia_t59_stats'
[init] Binding control socket '/tmp/qualia_t59.sock'...
[init] Owner-only mode: holding shared memory without spawning runners.

[init] 0 runners launched.
$ export QUALIA_SHM_NAME=/qualia_t59     # the only key either runner did not take from its defaults
$ timeout 15 ./target/release/qualia-lidar; echo LIDAR_RC=$?
qualia-lidar: opening serial port /dev/ttyACM0 @ 230400
qualia-lidar: open failed: Device or resource busy
LIDAR_RC=2
$ timeout 15 ./target/release/qualia-drive; echo DRIVE_RC=$?
qualia-drive: failed to open serial '/dev/ttyTHS1': Device or resource busy
DRIVE_RC=2
```

The manifest is the shipped `config/stack-manifest.default.json` with only `shared_memory.name`,
`stack_name` and `control.socket` renamed to the private region. The holder was killed at 03:02:39
(`/dev/shm/qualia_t59` and `qualia_t59_stats` removed, the board back at load 0.58, `leash` pid 1299
still holding its four fds) — nothing else on the board was created, opened or signalled.

| Runner | Kind | Command → observed |
| --- | --- | --- |
| `qualia-leash-sensors` | **device** (leash MCP, HTTP) | `QUALIA_LEASH_BASE_URL=http://192.168.55.1:8000 ./qualia-leash-sensors` → `qualia-leash-sensors: subscribing to http://192.168.55.1:8000/mcp observe every 100ms; publishing range scans into /qualia_t59` / `leash surface imu=available(waveshare-ugv) odometry=present` / `scan 1 source=waveshare-ugv-ld06 frame=base_scan points=360 valid=269 rate=10.000Hz range_mm=158..3395 age_ms=2220` … `scan 400` (real LD06, 10 Hz) |
| `qualia-camera` | **device** (leash MJPEG, HTTP) | `QUALIA_CAMERA_STREAM_URL=http://192.168.55.1:8000/camera/stream.mjpg ./qualia-camera` → `qualia-camera: consuming live MJPEG stream …` / `frame_seq=2 src=640x480 thumb=64x48 luma_mean=0.469 luma_std=0.149 quality=usable` … `frame_seq=362` |
| `qualia-lidar` | **device** (serial), refuses the leash's port | board, `99f56a5`, `QUALIA_SHM_NAME=/qualia_t59 ./qualia-lidar` with every other key at its default → `qualia-lidar: opening serial port /dev/ttyACM0 @ 230400` / `qualia-lidar: open failed: Device or resource busy` — exit code **2**, the documented "cannot open the port" (`LIDAR_RC=2`). The port is the leash's (`45uW`); superseded on this robot: it cannot open the port the leash holds. Its `publish_scan` is the one scan encoder the bridge reuses |
| `qualia-drive` | **device** (serial), refuses the leash's port | board, `99f56a5`, `QUALIA_SHM_NAME=/qualia_t59 ./qualia-drive` with `QUALIA_DRIVE_ARMED` unset (disarmed, so no speed frame is sent either way) → `qualia-drive: failed to open serial '/dev/ttyTHS1': Device or resource busy` — exit code **2** (`DRIVE_RC=2`). Defaults `/dev/ttyTHS1` @ `115200`, `QUALIA_DRIVE_TICK_MS=100`; the port is the leash's (`44uW`). The motion authority is the leash's; the agent reaches it over HTTP (`motion.navigate` → `POST /navigation/goals`) |
| `qualia-health` | no device (arena) | `QUALIA_SHM_NAME=/qualia_t59 ./qualia-health` → stdout is raw `HealthReport` frames at 10 Hz (32 B × `NUM_LAYERS` 8 = 256 B each, `#[repr(C)]`); a 6 s run'"'"'s captured output is 15 401 B — 15 360 B of frames (60 of them) plus the 41 B stderr line `qualia-health: opening shm '"'"'/qualia_t59'"'"'` |
| `qualia-vision` | fixture-only unless a Gemini key | `… ./qualia-vision` with no key → `qualia-vision: WARNING: GEMINI_API_KEY not set` / `Running in offline mode — synthetic world model only` / `offline tick 360, 2 objects, brightness=0.00`. Online path (key present): `using arena camera preview seq=16 640x480 (53740B, age=67ms)` → `calling Gemini Vision API (71656B image)...` — the frame is real and current; the model call needs the network and a key |
| `qualia-pose` | no device (arena) | `… ./qualia-pose` → `starting lidar pose graph from shm=/qualia_t59` / `initialized pose from first scan points=135` / `pose_seq=258 x_m=-0.004 z_m=-0.001 yaw_deg=-0.02 score=0.009 matches=133 keyframes=1` — ICP on the real scans the bridge published |
| `qualia-map` | no device (arena) | `… ./qualia-map` → `starting persistent lidar mapping from shm=/qualia_t59` / `map_seq=2 occupied_cells=60 observed_cells=604 bin_occ=31 bin_free=577 bin_unknown=64928` / `skip integration pose_conf=0.910 pose_age_ms=319 min_conf=0.700 max_age_ms=250` |
| `qualia-vslam` | no device (arena), not run | front end over the arena's camera frame; `cudarc` is an **optional** feature (`runners/vslam/Cargo.toml`), so a CPU build exists (`cuda_device_name()` returns `None`). Not built in this pass — the host was carrying three other cargo jobs; the row is unexercised |
| `qualia-explore` | no device (arena + planner socket) | `… ./qualia-explore` → `starting frontier selection shm=/qualia_t59` / `mission opened id=explore-frontier` / `frontier_clusters=5` / `planner_rejected_candidates=3 reasons={"goal_blocked": 3}` / `no reachable frontier found` — the planner answered over `QUALIA_COMPUTE_SOCKET`, so the socket path is live |
| `qualia-l1-belief`, `l2`, `l3` | no device (arena slot + compute backend) | `QUALIA_FLY_MODE=off` default, `QUALIA_FLY_PRIOR_PATH` for a coupling prior; the slot and the loop belong to `qualia-cuda` on the board / `qualia-metal` on Apple silicon, chosen at build time. Not built in this pass (no GPU backend build) |
| `qualia-l4-behavior`, `l5`, `l6-semantic` | no device (arena slot + compute backend) | same as l1–l3; the supervisor names each binary and reads its exit status. Not built in this pass |
| `qualia-l0-superposition` | no device (arena) | named by the default manifest; a thin layer boundary like the others |
| `qualia-jepa-runtime` | no device (arena + generation pointer) | refuses to run unless `QUALIA_JEPA_ENABLE` (and its sibling enable key) is set; reads `QUALIA_JEPA_GENERATION_FILE`, `QUALIA_JEPA_BACKEND`, `QUALIA_SHM_NAME`. Not built in this pass (its `candle` closure is the heavy one) |
| `qualia-arena-recorder` | no device (arena → MCAP) | `QUALIA_MCAP_DURATION_SECONDS=8 ./qualia-arena-recorder` → `recording session=arena-… sources=unknown:/qualia_t59(primary)` / `completed …\arena-….mcap sha256=3867eae6… channels=12` — 679 632 B |
| `qualia-init` | no device (owns the arena) | `QUALIA_STACK_MANIFEST=… ./qualia-init` → `Creating shared memory '/qualia_t59'...` / `Shared memory created: 64 MB` / `Stats region created: '/qualia_t59_stats'` / `4 runners launched.` |
| `qualia-cli` | no device (control socket + agent HTTP) | keys `QUALIA_SOCK_PATH`, `QUALIA_AGENT_URL`, `QUALIA_COMPUTE_SOCKET`, `QUALIA_WEB_PORT`; it speaks to a running stack rather than a device. Not run in this pass |
| `qualia-watch` | no device; **windowed** | starts the stack and shows it (`QUALIA_STACK_MANIFEST`, `QUALIA_SOCK_PATH`, `QUALIA_AGENT_URL`). It opens a window, which D-023 forbids on the operator's desktop; its evidence is T60's console capture, not this audit |
| `qualia-console` | no device; **windowed** | `QUALIA_SHM_NAME`, `QUALIA_AGENT_URL`, `QUALIA_AGENT_TLS_DIR`, `QUALIA_STACK_MANIFEST`; reads the arena and the agent, opens a window. Same D-023 rule; T60/T63/T64 carry its screenshots |
| `qualia-floor`, `qualia-abstraction`, `qualia-session`, `qualia-l0-superposition`, `qualia-mission-broker`, `qualia-cuda-*` | no device | arena consumers or auxiliary binaries (the cuda bench/service/smoke trio is host GPU tooling, the broker is HTTP). Not part of this ticket's runner list; no device path |

## The live run the device rows come from

```console
$ # host, private region, producers from the robot's leash
$ QUALIA_STACK_MANIFEST=<default manifest, region renamed> ./qualia-init
+------------------------------------------+
|QUALIA ENGINE v0.1.0                      |
|stack: qualia-default                     |
+------------------------------------------+

[init] Creating shared memory '/qualia_t59'...
[init] Shared memory created: 64 MB
[init] Stats region created: '/qualia_t59_stats'
[init] Binding control socket '/tmp/qualia_t59.sock'...
[init] Spawning qualia-l0-superposition...
[init] WARNING: qualia-l0-superposition: The system cannot find the file specified. (os error 2)
[init] Spawning qualia-l1-belief...
[init] WARNING: qualia-l1-belief: The system cannot find the file specified. (os error 2)
[init] Spawning qualia-l2-belief...
[init] WARNING: qualia-l2-belief: The system cannot find the file specified. (os error 2)
[init] Spawning qualia-l3-belief...
[init] WARNING: qualia-l3-belief: The system cannot find the file specified. (os error 2)
[init] Spawning qualia-l4-behavior...
[init] WARNING: qualia-l4-behavior: The system cannot find the file specified. (os error 2)
[init] Spawning qualia-l5-behavior...
[init] WARNING: qualia-l5-behavior: The system cannot find the file specified. (os error 2)
[init] Spawning qualia-l6-semantic...
[init] WARNING: qualia-l6-semantic: The system cannot find the file specified. (os error 2)
[init] Spawning qualia-health...
[init]   pid 29996
[init] Spawning qualia-vision...
[init]   pid 6448
[init] Spawning qualia-agent...
[init] WARNING: qualia-agent: The system cannot find the file specified. (os error 2)
[init] Spawning qualia-leash-sensors...
[init]   pid 25984
[init] Spawning qualia-camera...
[init]   pid 30596

[init] 4 runners launched.
[init] Run 'qualia-watch' in another terminal for the TUI.
```

That is the run's own log, verbatim and unabridged
(`C:/tmp/Impl240DeviceAudit/live/init.log`), not a condensed quote; the ordering it prints — arena and
stats region created, control socket bound, then the spawns — is what the audit claims in §"What the
default stack manifest starts".

The eight warnings are the seven belief layers (`qualia-l0`–`l6`) and the agent: none of those
binaries was built in this pass. `init` skipping a missing binary is its documented partial-stack
path, and it is itself an audit finding: on this host the GPU layers and the agent are the parts that
need a build the plain host path does not produce.

## What the default stack manifest starts

Before this ticket, `config/stack-manifest.default.json` named ten runners — l0–l6, health, vision,
agent — and **neither** of the two runners that read the robot: a freshly deployed stack published
nothing from the sensors into the arena, and the camera was started out of band. The manifest now
names `qualia-leash-sensors` and `qualia-camera` and carries the loopback MJPEG default, with the
operator's own `QUALIA_LEASH_BASE_URL`/`QUALIA_CAMERA_STREAM_URL` passed through per runner, so a
deployed stack subscribes to the leash's surface by default and an operator can still point it at a
leash that is not on loopback. `runners/lidar` and `runners/drive` are deliberately **not** in it:
their device is the leash's (see the table).

## Interface notes

D-008/D-009 make emitted lines and environment keys interface, so the two changes that touch them
are named here rather than left to a diff:

* **`runners/vision`'s capture line changes.** The reference's line named a snapshot file
  (`qualia-vision: using snapshot {path} ({}B)`) that no process in this tree writes:
  `/tmp/qualia_orin_snap.jpg`, still declared at `runners/agent/src/lib.rs:65` and still read by
  `media::orin_snapshot_get` (`media.rs:113`), but written by nothing here. (Its sibling
  `/tmp/qualia_snapshot.jpg` *is* written, by `media::snapshot_post` — which is why the file has to
  be named rather than the class described.) The new line is
  `qualia-vision: using arena camera preview seq={seq} {w}x{h} ({}B, age={}ms)`. What an operator
  reads is strictly more: the frame's sequence, size and age, and the error paths say which runner
  has to be started instead.
* **Two inert keys leave the agent.** `QUALIA_ORIN_CAMERA_DEVICE` and
  `QUALIA_ORIN_SNAPSHOT_INTERVAL_MS` (with `AgentConfig::orin`) were read into a struct no code read,
  because this implementation has no camera capture: the frame arrives through the arena
  (`perception::camera_source()` defaults to `qualia-shm:camera_frame`). The reference's agent ran
  ffmpeg on `/dev/video0` at that interval, which is the device path D-025 (then numbered D-023)
  replaced.
* **The default manifest gains two runners and one key.** `qualia-leash-sensors`,
  `qualia-camera`, `QUALIA_CAMERA_STREAM_URL` and the two pass-throughs; the keys are the reference's
  spelling and the runners are the two D-025 subscribers. One consequence, for whoever owns the
  console's reader set next: the Telemetry rows are derived from runner *names*
  (`SensingRunner::for_runner_name`, `apps/qualia-console/src/views/telemetry.rs:57`), and
  `qualia-leash-sensors` is not one of them although it publishes range scans through
  `qualia-lidar::publish_scan`. So a deployment named by `QUALIA_STACK_MANIFEST` now declares
  `["qualia-camera"]` — a camera row and no lidar row — while lidar frames are in the region. The
  default (no manifest named) is unaffected: it renders every ABI sensing slot.

## What is absent, and what hardware would fix it

| Absent | Consequence today | Hardware that would fix it |
| --- | --- | --- |
| IMU (no I2C responder, no IIO node) | the robot's only inertial data is `leash.sensors.imu`; no runner consumes it | an MPU/ICM/BNO-class IMU wired to an I2C bus the Tegra exposes, or a leash release that publishes IMU into the arena |
| Localization provider (`localization.pose: null`, provider `initializing`) | pose comes from `runners/pose`'s lidar ICP, not the leash | nothing to buy; the leash profile's provider needs to come up |
| Second serial adapters | only the LD06's CDC bridge and the Tegra UART exist; there is no spare `/dev/ttyUSB*` | a USB-serial adapter if a second device is ever added |
| OAK/MyriadX accelerator (`03e7:2485`, no driver, no node) | the device is inert on this image | a base image that binds a driver to it |
| `GEMINI_API_KEY` | `runners/vision` runs offline/synthetic (`vision: offline mode, no GEMINI_API_KEY`) | a key in the operator's environment |
| A GPU backend build for l1–l6 | the belief layers are built `metal` by default and need `--features cuda` for the Orin | nothing to buy; a build with the Orin's backend |

## What could not be exercised, plainly

1. **The `qualia-lidar`/`qualia-drive` success path.** Both ran as runners on the board in this
   ticket's second window (03:02 board-local, lease on [#240](https://github.com/superposition/qualia/issues/240),
   `99f56a5`), and the lines and exit codes are in the table above — but what they measured is the
   *refusal*: `Device or resource busy` → exit 2, which is the documented contract for a port the
   leash owns. The scan-assembly path and the motor-write path behind a successful open are still
   not exercised on the robot, and cannot be without taking a port from the leash, which D-025 rules
   out. What the earlier version of this item recorded — that the runners had not been run on the
   board at all — is the part now closed.
2. **`qualia-vslam`, `qualia-cli`, `qualia-jepa-runtime` and l0–l6 were not built or run.** Three
   other cargo jobs held the host during this pass; each row names the input it needs instead.
3. **`qualia-watch` and `qualia-console` were not launched**: they open a window, and D-023 forbids
   opening one on the operator's desktop. Their evidence is the console captures T60/T63/T64 carry.
4. **The online Gemini call itself fails** with a probe key (`status code 400`); what the run proves
   is the *frame source* — a 53 KB 640×480 JPEG, 67–303 ms old, taken from the arena slot the camera
   runner publishes — not the model's answer.
