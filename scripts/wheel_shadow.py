"""Bounded engineered wheel proposals from frozen vision and measured body state.

No actuator, authorization, bearer, or transport client exists in this program.
"""

from __future__ import annotations

import argparse
import copy
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import math
import os
from pathlib import Path
import stat
import subprocess
import sys
import threading
import time
import urllib.parse
import urllib.request
import uuid


BODY_MAX_AGE_MS = 1000
VISION_MAX_AGE_MS = 1500
SENSOR_SKEW_MS = 500
CLEARANCE_M = 0.25
CHECKPOINT = "d0e42857e738d0315897c2d50fde9eb3fb1a3fb1f071d55dfc13d53d72bccc3f"
EXPORT = "38264020d56f16dabca42af3ff1d0dbf6bc88c5384e34ce5b31b7062461541d1"


def number(value, label):
    if type(value) not in (int, float) or not math.isfinite(value):
        raise ValueError(f"{label} is missing or nonfinite")
    return float(value)


def integer(value, label, positive=True):
    if type(value) is not int or value < (1 if positive else 0):
        raise ValueError(f"{label} is not a valid integer")
    return value


def clamp(value, low, high):
    return max(low, min(high, value))


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise ValueError("sensor/vision redirects are refused")


class InputPoller:
    def __init__(self, url, stop):
        self.url, self.stop = url, stop
        self.lock = threading.Lock()
        self.sample = {"data": None, "error": "waiting for first observation"}
        threading.Thread(target=self.poll, daemon=True).start()

    def poll(self):
        opener = urllib.request.build_opener(NoRedirect)
        while not self.stop.is_set():
            try:
                request = urllib.request.Request(self.url, headers={"Cache-Control": "no-cache"})
                with opener.open(request, timeout=0.7) as response:
                    if response.status != 200:
                        raise ValueError(f"HTTP {response.status}")
                    raw = response.read(4 * 1024 * 1024 + 1)
                if len(raw) > 4 * 1024 * 1024:
                    raise ValueError("observation exceeds 4 MiB")
                data = json.loads(raw)
                json.dumps(data, allow_nan=False)
                sample = {"data": data, "received_ns": time.time_ns(), "received_mono": time.monotonic(), "error": None}
            except Exception as error:
                sample = {"data": None, "error": str(error)}
            with self.lock:
                self.sample = sample
            self.stop.wait(0.1)

    def get(self):
        with self.lock:
            return self.sample


def source_age(timestamp_ms, sample, now, limit, label):
    stamp = integer(timestamp_ms, label)
    received_ms = sample["received_ns"] / 1e6
    if stamp > received_ms + 100:
        raise ValueError(f"{label} is ahead of receiving clock")
    age = max(0, received_ms - stamp) + max(0, now - sample["received_mono"]) * 1000
    if age > limit:
        raise ValueError(f"{label} is stale ({age:.0f} ms)")
    return age, now + (limit - age) / 1000


def parse_vision(sample, now):
    if sample["data"] is None:
        raise ValueError("vision unavailable: " + sample["error"])
    value = sample["data"]
    if (value.get("schema") != "qualia.flyvis-cells.v1" or value.get("state") != "running"
            or value.get("activity_kind") != "graded_model_voltage" or value.get("parameters_frozen") is not True
            or value.get("controls_locked") is not True or value.get("error") is not None
            or value.get("cell_type") != "T4a"):
        raise ValueError("vision is not a fresh frozen T4a response")
    model, source, output = value["model"], value["source"], value["output"]
    if model.get("checkpoint_sha256") != CHECKPOINT or model.get("export_sha256") != EXPORT or model.get("nodes") != 45669:
        raise ValueError("vision model identity differs from the pinned export")
    run_id = value.get("run_id")
    if not isinstance(run_id, str) or not run_id:
        raise ValueError("vision run identity is absent")
    stamps = [integer(source.get("request_started_unix_ns"), "camera request"),
              integer(source.get("received_unix_ns"), "camera receipt"),
              integer(output.get("completed_unix_ns"), "neural completion"),
              integer(value.get("published_unix_ns"), "vision publication")]
    if stamps != sorted(stamps) or source["received_unix_ns"] != output.get("input_received_unix_ns"):
        raise ValueError("vision source and output timestamps are not linked")
    image_hash = source.get("jpeg_sha256")
    if (not isinstance(image_hash, str) or len(image_hash) != 64
            or any(c not in "0123456789abcdef" for c in image_hash)
            or image_hash != output.get("input_sha256")):
        raise ValueError("vision input/output image hashes do not match")
    ages, expiries = [], []
    for stamp, label in zip(stamps, ("camera request", "camera receipt", "neural completion", "vision publication")):
        age, expiry = source_age(stamp // 1000000, sample, now, VISION_MAX_AGE_MS, label)
        ages.append(age); expiries.append(expiry)
    producer = source.get("producer", {})
    acquisition = integer(producer.get("acquisition_started_unix_ms"), "camera acquisition")
    dequeue = integer(producer.get("dequeued_unix_ms"), "camera dequeue")
    sequence = integer(producer.get("sequence"), "camera sequence", positive=False)
    if producer.get("timestamp_basis") != "v4l2-monotonic" or not acquisition <= dequeue <= stamps[1] / 1e6:
        raise ValueError("camera producer acquisition is not verified and linked to this image")
    acquisition_age, acquisition_expiry = source_age(acquisition, sample, now, VISION_MAX_AGE_MS, "camera acquisition")
    expiries.append(acquisition_expiry)
    for reported, label in ((source.get("received_age_ms"), "camera age"), (output.get("input_age_ms"), "neural input age"),
                            (output.get("age_ms"), "neural output age")):
        age = number(reported, label) + (now - sample["received_mono"]) * 1000
        if age < 0 or age > VISION_MAX_AGE_MS:
            raise ValueError(f"{label} exceeds freshness budget")
        expiries.append(now + (VISION_MAX_AGE_MS - age) / 1000)
    cells = value.get("cells")
    if not isinstance(cells, list) or len(cells) != 721:
        raise ValueError("T4a requires 721 actual model cells")
    ids, mass, moment = set(), 0.0, 0.0
    for cell in cells:
        index = integer(cell.get("model_index"), "model index", positive=False)
        if index in ids or not 24039 <= index <= 24759:
            raise ValueError("T4a model indices are duplicated or outside the pinned population")
        ids.add(index)
        u, v = cell.get("u"), cell.get("v")
        if type(u) is not int or type(v) is not int or max(abs(u), abs(v), abs(u + v)) > 15:
            raise ValueError("cell is outside the actual retinal lattice")
        number(cell.get("voltage"), "voltage")
        change = abs(number(cell.get("delta_voltage"), "neural change"))
        mass += change
        moment += change * v
    if not math.isfinite(mass) or not math.isfinite(moment):
        raise ValueError("neural aggregate is nonfinite")
    centroid = moment / (15 * mass) if mass > 1e-12 else 0.0
    return {"run_id": run_id, "tick": integer(value.get("tick"), "neural tick", positive=False),
            "cell_type": "T4a", "input_sha256": image_hash, "input_received_unix_ns": stamps[1],
            "input_age_ms": ages[1], "output_completed_unix_ns": stamps[2], "output_age_ms": ages[2],
            "camera_sequence": sequence, "camera_acquisition_unix_ms": acquisition,
            "camera_dequeued_unix_ms": dequeue, "camera_acquisition_age_ms": acquisition_age,
            "camera_exposure_unix_ns": None, "camera_timestamp_basis": producer["timestamp_basis"],
            "centroid_horizontal": clamp(centroid, -1, 1), "mean_abs_delta": mass / len(cells),
            "checkpoint_sha256": CHECKPOINT, "export_sha256": EXPORT}, min(expiries)


class BodyHistory:
    def __init__(self):
        self.previous = None
        self.velocity = None

    def reset(self):
        self.previous, self.velocity = None, None

    def parse(self, sample, now):
        if sample["data"] is None:
            raise ValueError("body unavailable: " + sample["error"])
        value = sample["data"]
        sensors = value["sensors"]
        if any(sensors.get(name, {}).get("status") != "available" for name in ("imu", "range_scan", "odometry")):
            raise ValueError("required IMU, lidar, or odometry is unavailable")
        imu, scan, odom = sensors["imu"]["sample"], sensors["range_scan"]["sample"], value["odometry_pose"]["pose"]
        if imu.get("frame_id") != "base_link" or scan.get("frame_id") != "base_scan" or odom.get("frame_id") != "odom":
            raise ValueError("body sensor coordinate frame is not recognized")
        stamps = [integer(item.get("ts_ms"), label) for item, label in ((imu, "IMU time"), (scan, "lidar time"), (odom, "odometry time"))]
        ages, expiries = zip(*(source_age(stamp, sample, now, BODY_MAX_AGE_MS, label)
                              for stamp, label in zip(stamps, ("IMU", "lidar", "odometry"))))
        if max(stamps) - min(stamps) > SENSOR_SKEW_MS:
            raise ValueError("body sensor timestamps differ by more than 500 ms")
        gyro = [number(imu["angular_velocity_radps"].get(axis), "gyro " + axis) for axis in "xyz"]
        accel = [number(imu["linear_acceleration_mps2"].get(axis), "acceleration " + axis) for axis in "xyz"]
        pose = [number(odom.get(key), key) for key in ("x_m", "y_m", "yaw_rad")]
        if any(abs(x) > 100 for x in gyro + accel) or any(abs(x) > 1000000 for x in pose):
            raise ValueError("body values exceed numerical operating envelope")
        low, high = number(scan.get("range_min_m"), "lidar minimum"), number(scan.get("range_max_m"), "lidar maximum")
        ranges = scan.get("ranges_m")
        if not 0 <= low < high <= 100 or not isinstance(ranges, list) or not 1 <= len(ranges) <= 10000:
            raise ValueError("lidar range limits or count are invalid")
        valid = [float(x) for x in ranges if type(x) in (int, float) and math.isfinite(x) and low <= x <= high]
        if len(valid) * 2 < len(ranges):
            raise ValueError("fewer than half of lidar beams are valid")
        returns = [x for x in valid if x < high]
        nearest = min(returns) if returns else None
        if self.previous is not None:
            previous_ts, previous_pose = self.previous
            if stamps[2] < previous_ts:
                raise ValueError("odometry timestamp moved backwards")
            if stamps[2] > previous_ts:
                elapsed = (stamps[2] - previous_ts) / 1000
                if elapsed > 2:
                    self.velocity = None
                else:
                    speed = math.hypot(pose[0] - previous_pose[0], pose[1] - previous_pose[1]) / elapsed
                    yaw = ((pose[2] - previous_pose[2] + math.pi) % math.tau - math.pi) / elapsed
                    if not math.isfinite(speed) or not math.isfinite(yaw) or speed > 5 or abs(yaw) > 20:
                        raise ValueError("derived odometry velocity exceeds numerical envelope")
                    self.velocity = (speed, yaw)
        self.previous = (stamps[2], pose)
        norm = math.sqrt(sum(x * x for x in accel))
        holds = []
        if self.velocity is None:
            holds.append("waiting for two fresh odometry samples")
        elif abs(self.velocity[1] - gyro[2]) > 0.5:
            holds.append("IMU and odometry yaw disagree by more than 0.5 rad/s")
        if not 4 <= norm <= 16:
            holds.append("acceleration norm outside 4..16 m/s2 gravity envelope")
        if nearest is not None and nearest < CLEARANCE_M:
            holds.append(f"nearest lidar return {nearest:.3f} m is inside {CLEARANCE_M:.2f} m hold")
        return {"imu_ts_ms": stamps[0], "lidar_ts_ms": stamps[1], "odometry_ts_ms": stamps[2],
                "oldest_age_ms": max(ages), "sensor_skew_ms": max(stamps) - min(stamps),
                "gyro_yaw_radps": gyro[2], "odom_yaw_radps": self.velocity[1] if self.velocity else None,
                "odom_speed_mps": self.velocity[0] if self.velocity else None,
                "acceleration_norm_mps2": norm, "nearest_return_m": nearest,
                "valid_ranges": len(valid), "total_ranges": len(ranges)}, min(expiries), holds


def propose(vision, body, ceiling):
    strength = vision["mean_abs_delta"] / (vision["mean_abs_delta"] + 0.002)
    speed = body["odom_speed_mps"] or 0.0
    forward = 0.025 * (0.5 + 0.5 * strength) / (1 + speed / 0.05)
    neural = 0.025 * vision["centroid_horizontal"]
    damping = 0.008 * body["gyro_yaw_radps"]
    # Positive camera v is right. Positive differential L-R turns clockwise;
    # a positive CCW body gyro adds the opposing clockwise damping term.
    differential = clamp(neural + damping, -0.025, 0.025)
    values = {"left_mps": clamp(forward + differential, -ceiling, ceiling),
              "right_mps": clamp(forward - differential, -ceiling, ceiling),
              "neural_turn_mps": neural, "gyro_damping_mps": damping, "forward_mps": forward}
    if any(not math.isfinite(x) for x in values.values()):
        raise ValueError("wheel proposal is nonfinite")
    return values


class Status:
    def __init__(self, args):
        self.lock = threading.Lock()
        self.data = {"schema": "qualia.wheel-shadow.v1", "run_id": args.run_id, "state": "starting",
                     "tick": 0, "published_unix_ns": time.time_ns(), "deadline_unix_ns": time.time_ns() + int(args.seconds * 1e9),
                     "policy_kind": "engineered_frozen_vision_readout", "motion_output": False,
                     "transport_attached": False if args.frames_out != "-" else None,
                     "max_speed_mps": args.max_speed, "clearance_hold_m": CLEARANCE_M,
                     "vision": None, "body": None, "proposed": None, "emitted_frame": None,
                     "frame_written_unix_ns": None, "hold_reasons": ["waiting for observations"],
                     "decision_age_ms": None, "error": None}

    def update(self, **fields):
        with self.lock:
            self.data.update(fields)

    def snapshot(self):
        with self.lock:
            result = copy.deepcopy(self.data)
        result["published_unix_ns"] = time.time_ns()
        stamp = result.pop("_decision_mono", None)
        if stamp is not None:
            elapsed = max(0, (time.monotonic() - stamp) * 1000)
            result["decision_age_ms"] = elapsed
            if result["vision"]:
                result["vision"]["input_age_ms"] += elapsed
                result["vision"]["output_age_ms"] += elapsed
                result["vision"]["camera_acquisition_age_ms"] += elapsed
            if result["body"]:
                result["body"]["oldest_age_ms"] += elapsed
            if elapsed > 500 and result["state"] in ("shadow_proposal", "held"):
                result["state"] = "held"
                result["hold_reasons"].append("decision publisher is stale; last emitted frame is historical")
        return result


def start_http(status, port):
    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            if self.path != "/status":
                self.send_error(404)
                return
            raw = json.dumps(status.snapshot(), allow_nan=False).encode()
            try:
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(raw)))
                self.send_header("Cache-Control", "no-store")
                self.end_headers()
                self.wfile.write(raw)
            except OSError:
                pass

        def log_message(self, format, *args):
            pass
    server = ThreadingHTTPServer(("0.0.0.0", port), Handler)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def write_frame(fd, frame):
    raw = (json.dumps(frame, separators=(",", ":"), allow_nan=False) + "\n").encode()
    if os.write(fd, raw) != len(raw):
        raise OSError("incomplete drive-frame write; producer stops")


def run(args):
    stop = threading.Event()
    status = Status(args)
    server = start_http(status, args.port)
    fd = None
    tick = 0
    try:
        if args.frames_out == "-":
            fd = sys.stdout.fileno()
            if stat.S_ISFIFO(os.fstat(fd).st_mode):
                os.set_blocking(fd, False)
        else:
            fd = os.open(args.frames_out, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        vision_poll = InputPoller(args.vision_url, stop)
        body_poll = InputPoller(args.base_url + "/telemetry/compact", stop)
        history = BodyHistory()
        previous_run = None
        deadline = time.monotonic() + args.seconds - 1
        next_tick = time.monotonic()
        with (args.run_dir / "decisions.jsonl").open("x", buffering=1) as evidence:
            while time.monotonic() < deadline:
                now = time.monotonic()
                vision = body = proposal = None
                expiry = now
                holds = []
                try:
                    vision, vision_expiry = parse_vision(vision_poll.get(), now)
                    if previous_run != vision["run_id"]:
                        history.reset()
                        previous_run = vision["run_id"]
                except Exception as error:
                    holds.append(str(error))
                    history.reset()
                try:
                    body, body_expiry, body_holds = history.parse(body_poll.get(), now)
                    holds.extend(body_holds)
                except Exception as error:
                    holds.append(str(error))
                    history.reset()
                if vision is not None and body is not None:
                    expiry = min(vision_expiry, body_expiry)
                    if body["imu_ts_ms"] + SENSOR_SKEW_MS < vision["input_received_unix_ns"] / 1e6:
                        holds.append("body observation predates camera input by more than 500 ms")
                    if vision["mean_abs_delta"] <= 1e-9:
                        holds.append("no measured spatial neural change")
                    try:
                        proposal = propose(vision, body, args.max_speed)
                    except Exception as error:
                        holds.append(str(error))
                if time.monotonic() >= expiry:
                    holds.append("inputs expired or missing before frame serialization")
                left, right = (0.0, 0.0) if holds or proposal is None else (proposal["left_mps"], proposal["right_mps"])
                frame = {"T": tick, "L": left, "R": right}
                write_frame(fd, frame)
                status.update(state="held" if holds else "shadow_proposal", tick=tick, vision=vision, body=body,
                              proposed=proposal, emitted_frame=frame, hold_reasons=holds, error=None,
                              frame_written_unix_ns=time.time_ns(), _decision_mono=now)
                snapshot = status.snapshot()
                evidence.write(json.dumps(snapshot, allow_nan=False) + "\n")
                temporary = args.run_dir / "status.tmp"
                temporary.write_text(json.dumps(snapshot, allow_nan=False) + "\n")
                os.replace(temporary, args.run_dir / "status.json")
                tick += 1
                next_tick += args.period_ms / 1000
                stop.wait(max(0, next_tick - time.monotonic()))
                if next_tick < time.monotonic() - args.period_ms / 1000:
                    next_tick = time.monotonic()
        final = {"T": tick, "L": 0.0, "R": 0.0}
        write_frame(fd, final)
        status.update(state="stopped", tick=tick, emitted_frame=final, frame_written_unix_ns=time.time_ns(),
                      hold_reasons=["bounded shadow interval ended"], _decision_mono=time.monotonic())
        with (args.run_dir / "decisions.jsonl").open("a") as evidence:
            evidence.write(json.dumps(status.snapshot(), allow_nan=False) + "\n")
        return 0
    except Exception as error:
        if fd is not None:
            try:
                final = {"T": tick, "L": 0.0, "R": 0.0}
                write_frame(fd, final)
                status.update(tick=tick, emitted_frame=final, frame_written_unix_ns=time.time_ns(),
                              _decision_mono=time.monotonic())
            except OSError:
                pass
        status.update(state="failed", error=str(error), hold_reasons=["shadow producer failed"])
        print(f"wheel shadow failed: {error}", file=sys.stderr)
        return 1
    finally:
        stop.set()
        (args.run_dir / "status.json").write_text(json.dumps(status.snapshot(), allow_nan=False) + "\n")
        if fd is not None and args.frames_out != "-":
            os.close(fd)
        server.shutdown()
        server.server_close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base-url", default="http://127.0.0.1:8000")
    parser.add_argument("--vision-url", default="http://127.0.0.1:8091/cells?type=T4a")
    parser.add_argument("--run-dir", type=Path, required=True)
    parser.add_argument("--frames-out", default=None)
    parser.add_argument("--seconds", type=int, default=60)
    parser.add_argument("--period-ms", type=int, default=200)
    parser.add_argument("--max-speed", type=float, default=0.05)
    parser.add_argument("--port", type=int, default=8092)
    parser.add_argument("--run-id", default=None)
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    args.base_url = args.base_url.rstrip("/")
    for url in (args.base_url, args.vision_url):
        parsed = urllib.parse.urlsplit(url)
        if parsed.scheme not in ("http", "https") or not parsed.hostname or parsed.username or parsed.password or parsed.fragment:
            parser.error("input endpoints require HTTP(S) without credentials")
    if urllib.parse.urlsplit(args.base_url).path or urllib.parse.urlsplit(args.base_url).query:
        parser.error("base-url must be an HTTP origin")
    if not 10 <= args.seconds <= 1800 or not 50 <= args.period_ms <= 250 or not 1024 <= args.port <= 65535:
        parser.error("seconds10..1800,period-ms50..250,port1024..65535 required")
    if not math.isfinite(args.max_speed) or not 0 <= args.max_speed <= 0.05:
        parser.error("max-speed must be finite in0..0.05m/s")
    args.run_id = args.run_id or str(uuid.uuid4())
    args.frames_out = args.frames_out or str(args.run_dir / "frames.jsonl")
    if args.worker:
        return run(args)
    if args.frames_out != "-" and Path(args.frames_out).exists():
        parser.error("existing frame destination is refused")
    if args.frames_out != "-" and Path(args.frames_out).resolve() in {
            (args.run_dir / name).resolve() for name in ("status.json", "status.tmp", "decisions.jsonl")}:
        parser.error("frame destination aliases an evidence file")
    args.run_dir.mkdir(parents=True, exist_ok=False)
    command = [sys.executable, str(Path(__file__).resolve()), *sys.argv[1:], "--worker", "--run-id", args.run_id]
    try:
        return subprocess.run(command, timeout=args.seconds,
                              creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0)).returncode
    except subprocess.TimeoutExpired:
        (args.run_dir / "status.json").write_text(json.dumps({"schema": "qualia.wheel-shadow.v1", "run_id": args.run_id,
                                                            "state": "stopped", "error": "supervisor deadline ended"}) + "\n")
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
