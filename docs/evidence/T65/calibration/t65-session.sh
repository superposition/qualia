#!/usr/bin/env bash
# T65 (#250) board leg: a real session whose camera records carry a measured
# calibration identity, with a real pose stream and a real transport ledger.
#
#   t65-session.sh <session-name> <duration-seconds>
#
# The chain, all of it the robot's own traffic:
#   qualia-init          creates /qualia_body
#   qualia-camera        the leash's MJPEG stream (/dev/video0 is leash's)
#   qualia-leash-sensors the leash's LD06 rotation (/dev/ttyACM0 is leash's)
#   qualia-pose          ICP over those real scans -> NavPose.confidence
#   qualia-leash-transport  the leash's acknowledged zero-speed stop -> the
#                           applied-action ledger, authority=leash, no motion
#   qualia-calibration   observes the two slots and the leash's own declaration,
#                        emits calibration.json and the id the recorder stamps
#   qualia-arena-recorder  arena -> sealed MCAP under that calibration id
#
# Nothing opens a device the leash owns; every command here is HTTP against the
# leash's MCP surface or a read of our own shared memory.
set -u
export PATH="$HOME/.cargo/bin:/usr/local/cuda/bin:$PATH"
TREE="$HOME/qualia-deploy/T65"
BIN="$TREE/target/release"
ROOT="$HOME/t65-capture"
SESSION="${1:-t65-real-manual}"
DURATION="${2:-300}"
CALIBRATION_WINDOW="${3:-8}"
LEASH_URL="${QUALIA_LEASH_BASE_URL:-http://127.0.0.1:8000}"

RUN="$ROOT/$SESSION"
rm -rf "$RUN"; mkdir -p "$RUN/logs" "$RUN/mcap"

echo "== device enumeration =="
ls -l /dev/video* 2>&1
ls -l /dev/ttyUSB* /dev/ttyACM* 2>&1
ls -l /dev/iio:device* 2>&1
v4l2-ctl --list-devices 2>&1 | sed -n '1,8p'
echo "== leash owner of the ports =="
echo jetson | sudo -S lsof /dev/ttyACM0 /dev/ttyTHS1 2>&1 | tail -5
echo "== leash health =="
curl -s -m 5 -X POST "$LEASH_URL/mcp" -H "content-type: application/json" \
  -d '{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"health","arguments":{}}}' \
  | tr ',' '\n' | grep -E '"(mode|deadman_ok|estop)"' | head -5
echo "== leash sensor surface (fresh observe) =="
curl -s -m 5 -X POST "$LEASH_URL/mcp" -H "content-type: application/json" \
  -d '{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"observe","arguments":{}}}' \
  > "$RUN/leash-observe.json" 2>&1

START_NS=$(date +%s%N)
echo "== start $SESSION at $START_NS ($(date -u -d @$((START_NS/1000000000)) +%Y-%m-%dT%H:%M:%SZ)) for ${DURATION}s =="

QUALIA_SHM_NAME=/qualia_body QUALIA_INIT_OWNER_ONLY=1 "$BIN/qualia-init" \
  > "$RUN/logs/init.log" 2>&1 &
INIT_PID=$!
sleep 2

QUALIA_SHM_NAME=/qualia_body QUALIA_CAMERA_STREAM_URL="$LEASH_URL/camera/stream.mjpg" \
  QUALIA_CAMERA_POLL_MS=100 "$BIN/qualia-camera" > "$RUN/logs/camera.log" 2>&1 &
CAM_PID=$!

QUALIA_SHM_NAME=/qualia_body QUALIA_LEASH_BASE_URL="$LEASH_URL" \
  "$BIN/qualia-leash-sensors" > "$RUN/logs/leash-sensors.log" 2>&1 &
SENSORS_PID=$!

QUALIA_SHM_NAME=/qualia_body "$BIN/qualia-pose" > "$RUN/logs/pose.log" 2>&1 &
POSE_PID=$!

QUALIA_SHM_NAME=/qualia_body QUALIA_LEASH_BASE_URL="$LEASH_URL" \
  "$BIN/qualia-leash-transport" > "$RUN/logs/leash-transport.log" 2>&1 &
TRANSPORT_PID=$!

# Let the streams establish before the calibration window opens: the producer
# measures live traffic, so it must not run against empty slots.
sleep 5

echo "== qualia-calibration (${CALIBRATION_WINDOW}s window) =="
QUALIA_SHM_NAME=/qualia_body QUALIA_LEASH_BASE_URL="$LEASH_URL" \
  QUALIA_CALIBRATION_OUT="$RUN/calibration.json" \
  QUALIA_CALIBRATION_WINDOW_SECONDS="$CALIBRATION_WINDOW" \
  QUALIA_CALIBRATION_ENTITY=pinkie \
  "$BIN/qualia-calibration" 2>&1 | tee "$RUN/logs/calibration.log"
CAL_RC=${PIPESTATUS[0]}
echo "CALIBRATION_RC=$CAL_RC"
CAL_ID=$(grep -o 'calibration_id=[0-9a-f][0-9a-f]*' "$RUN/logs/calibration.log" | head -1 | cut -d= -f2)
echo "CALIBRATION_ID=$CAL_ID"
if [ -z "$CAL_ID" ]; then
  echo "NO_CALIBRATION_ID: the producer measured no identity, so the recorder must not be started"
  kill "$TRANSPORT_PID" "$POSE_PID" "$SENSORS_PID" "$CAM_PID" "$INIT_PID" 2>/dev/null
  wait "$TRANSPORT_PID" "$POSE_PID" "$SENSORS_PID" "$CAM_PID" "$INIT_PID" 2>/dev/null
  exit 4
fi

echo "== qualia-arena-recorder (${DURATION}s, calibration_id=$CAL_ID) =="
QUALIA_SHM_NAME=/qualia_body QUALIA_MCAP_ROOT="$RUN/mcap" \
  QUALIA_ARENA_SESSION="$SESSION" QUALIA_MCAP_DURATION_SECONDS="$DURATION" \
  QUALIA_CALIBRATION_ID="$CAL_ID" QUALIA_CAMERA_ENTITY=pinkie \
  "$BIN/qualia-arena-recorder" > "$RUN/logs/recorder.log" 2>&1
RECORDER_RC=$?
echo "RECORDER_RC=$RECORDER_RC"

kill "$TRANSPORT_PID" "$POSE_PID" "$SENSORS_PID" "$CAM_PID" "$INIT_PID" 2>/dev/null
wait "$TRANSPORT_PID" "$POSE_PID" "$SENSORS_PID" "$CAM_PID" "$INIT_PID" 2>/dev/null
END_NS=$(date +%s%N)

echo "== session files =="
ls -l "$RUN/mcap"
MCAP=$(ls "$RUN"/mcap/*.mcap 2>/dev/null | head -1)
echo "MCAP=$MCAP"
if [ -n "$MCAP" ]; then
  sha256sum "$MCAP" | tee "$RUN/mcap.sha256"
  stat -c '%s' "$MCAP" | tee "$RUN/mcap.size"
  echo "== qualia-mcap-inspect =="
  "$BIN/qualia-mcap-inspect" "$MCAP" 2>&1 | tee "$RUN/logs/inspect.log"
  echo "INSPECT_RC=${PIPESTATUS[0]}"
fi

echo "== recorder log =="
cat "$RUN/logs/recorder.log"
echo "== leash-transport log (first and last three lines) =="
sed -n '1,4p' "$RUN/logs/leash-transport.log"; echo "..."; tail -3 "$RUN/logs/leash-transport.log"
echo "== pose log (first and last three lines) =="
sed -n '1,4p' "$RUN/logs/pose.log"; echo "..."; tail -3 "$RUN/logs/pose.log"
echo "== window =="
echo "start_ns=$START_NS end_ns=$END_NS elapsed_s=$(( (END_NS-START_NS)/1000000000 ))"
echo "$START_NS $END_NS $DURATION" > "$RUN/window.txt"
echo "== end =="
