#!/usr/bin/env bash
# T58 (#239) board leg: capture the robot's own live sensors into MCAP.
#
#   t58-capture.sh <session-name> <duration-seconds>
#
# Sensors taken: the camera stream and the LD06 range scan the leash serves,
# via the stack's own runners (qualia-camera, qualia-leash-sensors) into the
# arena, then qualia-arena-recorder seals the MCAP. Nothing is synthesized and
# no device the leash owns is opened twice.
set -u
export PATH="$HOME/.cargo/bin:/usr/local/cuda/bin:$PATH"
TREE="$HOME/qualia-deploy/T58c"
BIN="$TREE/target/release"
ROOT="$HOME/t58-capture"
SESSION="${1:-t58-real-manual}"
DURATION="${2:-900}"
LEASH_URL="${QUALIA_LEASH_BASE_URL:-http://127.0.0.1:8000}"

RUN="$ROOT/$SESSION"
rm -rf "$RUN"; mkdir -p "$RUN/logs" "$RUN/mcap"

echo "== device enumeration =="
ls -l /dev/video* 2>&1
ls -l /dev/ttyUSB* /dev/ttyACM* 2>&1
ls -l /dev/iio:device* 2>&1
v4l2-ctl --list-devices 2>&1 | sed -n '1,8p'
timeout 3 tegrastats 2>&1 | head -2
echo "== leash owner of the ports =="
echo jetson | sudo -S lsof /dev/ttyACM0 /dev/ttyTHS1 2>&1 | tail -5
echo "== leash sensor surface (fresh observe) =="
curl -s -m 5 -X POST "$LEASH_URL/mcp" -H "content-type: application/json" \
  -d '{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"observe","arguments":{}}}' \
  > "$RUN/leash-observe.json" 2>&1
python3 - "$RUN/leash-observe.json" <<'PY'
import json,sys
d=json.load(open(sys.argv[1]))["result"]["content"][0]["text"]
o=json.loads(d)
s=o["sensors"]
print("camera:", json.dumps(s.get("camera")))
print("range_scan:", s["range_scan"]["status"], s["range_scan"]["source"], "last_ms", s["range_scan"]["last_ms"],
      "points", len(s["range_scan"]["sample"]["ranges_m"]) if s["range_scan"].get("sample") else 0)
print("imu:", s["imu"]["status"], s["imu"]["source"], "last_ms", s["imu"]["last_ms"])
print("odometry:", json.dumps(s.get("odometry")))
PY

START_NS=$(date +%s%N)
echo "== start $SESSION at $START_NS ($(date -u -d @$((START_NS/1000000000)) +%Y-%m-%dT%H:%M:%SZ)) for ${DURATION}s =="

# The arena's owner: creates /qualia_body and holds it without spawning children.
QUALIA_SHM_NAME=/qualia_body QUALIA_INIT_OWNER_ONLY=1 "$BIN/qualia-init" \
  > "$RUN/logs/init.log" 2>&1 &
INIT_PID=$!
sleep 2

# The camera: the leash serves /dev/video0 as MJPEG; the runner's own stream path reads it.
QUALIA_SHM_NAME=/qualia_body QUALIA_CAMERA_STREAM_URL="$LEASH_URL/camera/stream.mjpg" \
  QUALIA_CAMERA_POLL_MS=100 "$BIN/qualia-camera" > "$RUN/logs/camera.log" 2>&1 &
CAM_PID=$!

# The body's sensors the leash owns: range scan into the arena's lidar slot.
QUALIA_SHM_NAME=/qualia_body QUALIA_LEASH_BASE_URL="$LEASH_URL" \
  "$BIN/qualia-leash-sensors" > "$RUN/logs/leash-sensors.log" 2>&1 &
SENSORS_PID=$!

sleep 3

# The recorder: arena -> sealed MCAP, bounded.
QUALIA_SHM_NAME=/qualia_body QUALIA_MCAP_ROOT="$RUN/mcap" \
  QUALIA_ARENA_SESSION="$SESSION" QUALIA_MCAP_DURATION_SECONDS="$DURATION" \
  QUALIA_MCAP_SOURCES_JSON='[{"entity":"pinkie","shm_name":"/qualia_body","calibration_id":"unavailable","primary":true}]' \
  "$BIN/qualia-arena-recorder" > "$RUN/logs/recorder.log" 2>&1
RECORDER_RC=$?
echo "RECORDER_RC=$RECORDER_RC"

kill "$CAM_PID" "$SENSORS_PID" "$INIT_PID" 2>/dev/null
wait "$CAM_PID" "$SENSORS_PID" "$INIT_PID" 2>/dev/null
END_NS=$(date +%s%N)

echo "== session files =="
ls -l "$RUN/mcap"
MCAP=$(ls "$RUN"/mcap/*.mcap 2>/dev/null | head -1)
echo "MCAP=$MCAP"
if [ -n "$MCAP" ]; then
  sha256sum "$MCAP" | tee "$RUN/mcap.sha256"
  stat -c '%n %s bytes' "$MCAP" | tee "$RUN/mcap.size"
  echo "== inspect =="
  "$BIN/qualia-mcap-inspect" "$MCAP" > "$RUN/inspect.json" 2> "$RUN/inspect.err"
  echo "INSPECT_RC=$?"
  python3 - "$RUN/inspect.json" <<'PY'
import json,sys
d=json.load(open(sys.argv[1]))
for s in d.get("streams", []):
    print(f'{s["topic"]:24} {s["entity"]:8} messages={s["messages"]:6} span_ns={s["max_timestamp_ns"]-s["min_timestamp_ns"]:>14} payload_bytes={s["payload_bytes"]}')
print("session_span_ns", d.get("span_ns"), "total_payload_bytes", d.get("total_payload_bytes"))
PY
fi
echo "== recorder log =="
cat "$RUN/logs/recorder.log"
echo "== camera log (first and last three lines) =="
sed -n '1,3p' "$RUN/logs/camera.log"; echo "..."; tail -3 "$RUN/logs/camera.log"
echo "== leash-sensors log (first and last three lines) =="
sed -n '1,3p' "$RUN/logs/leash-sensors.log"; echo "..."; tail -3 "$RUN/logs/leash-sensors.log"
echo "== window =="
echo "start_ns=$START_NS end_ns=$END_NS elapsed_s=$(( (END_NS-START_NS)/1000000000 ))"
echo "$START_NS $END_NS $DURATION" > "$RUN/window.txt"
echo "== end =="
