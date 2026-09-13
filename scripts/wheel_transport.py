"""Bounded attachment of fresh wheel decisions; zero-only unless explicitly enabled.

Only the child qualia-leash-transport reads the operator credential or writes to
the robot. This bridge has no authorization, drive, stop, or estop HTTP client.
"""

from __future__ import annotations

import argparse
import copy
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import math
import os
from pathlib import Path
import re
import subprocess
import threading
import time
import urllib.parse
import urllib.error
import urllib.request
import uuid

from wheel_shadow import CHECKPOINT, EXPORT, NoRedirect, integer, number


FRESH_MS = 500


class InterlockViolation(ValueError):
    pass


def named_error(error):
    if isinstance(error, urllib.error.HTTPError):
        return f"source HTTP {error.code}"
    if isinstance(error, urllib.error.URLError):
        return f"source connection failed ({type(error.reason).__name__})"
    if isinstance(error, (ValueError, KeyError, TypeError, RuntimeError)):
        return f"{type(error).__name__}: {str(error)[:200]}"
    if isinstance(error, OSError):
        return f"{type(error).__name__} errno={error.errno}"
    return type(error).__name__


def get_json(url):
    opener = urllib.request.build_opener(NoRedirect)
    with opener.open(urllib.request.Request(url, headers={"Cache-Control": "no-cache"}), timeout=0.7) as response:
        if response.status != 200:
            raise ValueError("read-only source HTTP failure")
        raw = response.read(2 * 1024 * 1024 + 1)
    if len(raw) > 2 * 1024 * 1024:
        raise ValueError("read-only source exceeds byte budget")
    value = json.loads(raw)
    json.dumps(value, allow_nan=False)
    return value


def current_frame(value):
    if (value.get("schema") != "qualia.wheel-shadow.v1"
            or value.get("policy_kind") != "engineered_frozen_vision_readout"
            or value.get("state") not in ("held", "shadow_proposal")
            or value.get("motion_output") is not False or value.get("error") is not None):
        raise ValueError("producer has no current wheel decision")
    run_id = value.get("run_id")
    if not isinstance(run_id, str) or not 1 <= len(run_id) <= 128:
        raise ValueError("producer run identity is invalid")
    tick = integer(value.get("tick"), "producer tick", positive=False)
    frame = value.get("emitted_frame")
    if not isinstance(frame, dict) or set(frame) != {"T", "L", "R"} or frame["T"] != tick:
        raise ValueError("producer frame does not match its evidence tick")
    integer(frame["T"], "frame tick", positive=False)
    number(frame["L"], "left"); number(frame["R"], "right")
    now = time.time_ns()
    published = integer(value.get("published_unix_ns"), "producer publication")
    written = integer(value.get("frame_written_unix_ns"), "producer frame write")
    deadline = integer(value.get("deadline_unix_ns"), "producer deadline")
    if not written <= published or any(not -100 <= (now - stamp) / 1e6 <= FRESH_MS for stamp in (written, published)):
        raise ValueError("producer frame/publication is stale or clock-skewed")
    age = number(value.get("decision_age_ms"), "producer decision age")
    if not 0 <= age + max(0, (now - published) / 1e6) <= FRESH_MS or deadline <= now:
        raise ValueError("producer decision expired")
    if not isinstance(value.get("hold_reasons"), list):
        raise ValueError("producer hold reasons are absent")
    return frame


def permitted_frame(value, motion_authorized, max_speed=0.05):
    frame = current_frame(value)
    if frame["L"] == 0 and frame["R"] == 0:
        return frame
    if not motion_authorized:
        raise InterlockViolation("zero-only interlock rejected a nonzero frame; no substitution")
    if max(abs(frame["L"]), abs(frame["R"])) > max_speed:
        raise InterlockViolation("motion interlock rejected a wheel speed above the configured cap")
    if value["state"] != "shadow_proposal" or value["hold_reasons"]:
        raise InterlockViolation("motion interlock rejected nonzero output with producer holds")
    if value.get("clearance_hold_m") != 0.25 or not 0 < number(value.get("max_speed_mps"), "producer speed cap") <= max_speed:
        raise InterlockViolation("producer clearance or wheel cap differs from the approved envelope")
    body, vision, proposal = value.get("body"), value.get("vision"), value.get("proposed")
    if not all(isinstance(item, dict) for item in (body, vision, proposal)):
        raise ValueError("motion requires original body, vision, and proposal evidence")
    if frame["L"] != proposal.get("left_mps") or frame["R"] != proposal.get("right_mps"):
        raise InterlockViolation("nonzero frame differs from its original proposal")
    now = time.time_ns()
    transit_ms = max(0, (now - value["published_unix_ns"]) / 1e6)

    def stamp_age(stamp, limit, label, nanos=False):
        age = (now - integer(stamp, label) * (1 if nanos else 1000000)) / 1e6
        if not -100 <= age <= limit:
            raise ValueError(label + " is stale or clock-skewed")

    stamps = [body.get(key) for key in ("imu_ts_ms", "lidar_ts_ms", "odometry_ts_ms")]
    for stamp, label in zip(stamps, ("original IMU", "original lidar", "original odometry")):
        stamp_age(stamp, 1000, label)
    if max(stamps) - min(stamps) > 500 or not 0 <= number(body.get("oldest_age_ms"), "body age") + transit_ms <= 1000:
        raise ValueError("body source age or sensor skew exceeds its budget")
    gyro, odom = number(body.get("gyro_yaw_radps"), "gyro yaw"), number(body.get("odom_yaw_radps"), "odometry yaw")
    if abs(gyro) > 100 or abs(odom) > 20 or abs(gyro - odom) > 0.5 or not 0 <= number(body.get("odom_speed_mps"), "odometry speed") <= 5:
        raise InterlockViolation("body yaw agreement or odometry speed is outside envelope")
    if not 4 <= number(body.get("acceleration_norm_mps2"), "acceleration norm") <= 16:
        raise InterlockViolation("body acceleration is outside gravity-inclusive envelope")
    valid = integer(body.get("valid_ranges"), "valid lidar ranges")
    total = integer(body.get("total_ranges"), "total lidar ranges")
    if not valid <= total <= 10000 or valid * 2 < total:
        raise InterlockViolation("lidar has insufficient valid measurements")
    if "nearest_return_m" not in body:
        raise ValueError("nearest lidar observation is absent")
    nearest = body["nearest_return_m"]
    if proposal.get("clearance_sector") == "forward":
        if (value.get("directional_clearance_enabled") is not True or body.get("forward_directional_eligible") is not True
                or proposal.get("direction") != "forward" or not 0 <= frame["L"] <= max_speed
                or not 0 <= frame["R"] <= max_speed):
            raise InterlockViolation("directional forward requires positive wheels within the configured cap")
        front = body.get("forward_sector")
        if not isinstance(front, dict) or front.get("center_deg") != 0 or front.get("half_width_deg") != 60:
            raise InterlockViolation("forward scan sector geometry differs")
        beams = integer(front.get("total_beams"), "forward beam count")
        seen = integer(front.get("valid_beams"), "forward valid count", positive=False)
        coverage = number(front.get("coverage_fraction"), "forward coverage")
        if (not 0 <= seen <= beams <= total or abs(coverage - seen / beams) > 1e-6 or coverage < 0.75
                or number(front.get("min_return_m"), "forward nearest return") < 0.25
                or proposal.get("selected_clearance_m") != front["min_return_m"]):
            raise InterlockViolation("forward motion lacks sufficient measured directional clearance")
    elif proposal.get("direction") == "reverse_escape":
        # Preserve the producer's emitted frame; a held zero stays zero.
        if (value.get("reverse_escape_enabled") is not True or body.get("reverse_escape_eligible") is not True
                or proposal.get("clearance_sector") != "reverse" or not -0.02 <= frame["L"] <= 0
                or not -0.02 <= frame["R"] <= 0):
            raise InterlockViolation("reverse escape requires both wheels reverse within 0.02 m/s")
        forward, rear = body.get("forward_sector"), body.get("reverse_sector")
        if not isinstance(forward, dict) or not isinstance(rear, dict):
            raise InterlockViolation("reverse escape lacks measured directional clearance")
        for sector, center in ((forward, 0), (rear, 180)):
            if sector.get("center_deg") != center or sector.get("half_width_deg") != 60:
                raise InterlockViolation("directional scan sector geometry differs")
            beams = integer(sector.get("total_beams"), "sector beam count")
            seen = integer(sector.get("valid_beams"), "sector valid count", positive=False)
            coverage = number(sector.get("coverage_fraction"), "sector coverage")
            if not 0 <= seen <= beams <= total or abs(coverage - seen / beams) > 1e-6:
                raise InterlockViolation("directional lidar coverage is inconsistent")
        if (number(forward.get("min_return_m"), "forward nearest return") >= 0.25
                or rear["coverage_fraction"] < 0.75
                or number(rear.get("min_return_m"), "reverse nearest return") < 0.25
                or proposal.get("selected_clearance_m") != rear["min_return_m"]):
            raise InterlockViolation("reverse escape is not supported by current directional clearance")
    elif nearest is not None and number(nearest, "nearest lidar return") < 0.25:
        raise InterlockViolation("nearest lidar return is inside the 0.25 m forward hold")
    if (vision.get("checkpoint_sha256") != CHECKPOINT or vision.get("export_sha256") != EXPORT
            or vision.get("cell_type") != "T4a" or not vision.get("run_id")
            or vision.get("camera_timestamp_basis") != "v4l2-monotonic"):
        raise InterlockViolation("vision identity or actual camera timestamp basis differs")
    image_hash = vision.get("input_sha256")
    if not isinstance(image_hash, str) or re.fullmatch("[0-9a-f]{64}", image_hash) is None:
        raise ValueError("original neural input hash is absent")
    for key in ("input_received_unix_ns", "output_completed_unix_ns"):
        stamp_age(vision.get(key), 1500, key, nanos=True)
    for key in ("camera_acquisition_unix_ms", "camera_dequeued_unix_ms"):
        stamp_age(vision.get(key), 1500, key)
    if not vision["camera_acquisition_unix_ms"] <= vision["camera_dequeued_unix_ms"] <= vision["input_received_unix_ns"] / 1e6:
        raise ValueError("camera acquisition and neural input are not causally ordered")
    if vision["input_received_unix_ns"] > vision["output_completed_unix_ns"]:
        raise ValueError("neural output predates its input")
    for key in ("input_age_ms", "output_age_ms", "camera_acquisition_age_ms"):
        if not 0 <= number(vision.get(key), key) + transit_ms <= 1500:
            raise ValueError("original " + key + " exceeds its budget")
    if stamps[0] + 500 < vision["input_received_unix_ns"] / 1e6:
        raise ValueError("body source predates the neural input by more than 500 ms")
    if number(vision.get("mean_abs_delta"), "measured neural change") <= 1e-9:
        raise InterlockViolation("no measured neural change supports the nonzero proposal")
    return frame


class State:
    def __init__(self, args):
        self.lock = threading.RLock()
        self.stop = threading.Event()
        self.abort = threading.Event()
        self.process = None
        self.pending = None
        self.last_consumed_mono = None
        self.data = {
            "schema": "qualia.wheel-transport.v1", "run_id": str(uuid.uuid4()), "state": "starting",
            "published_unix_ns": time.time_ns(), "deadline_unix_ns": time.time_ns() + int(args.seconds * 1e9),
            "zero_only": not args.motion_authorized, "nonzero_enabled": args.motion_authorized,
            "max_wheel_speed_mps": args.max_speed, "transport_attached": False, "error": None,
            "frame_source": "producer emitted frame",
            "source_failures": 0, "source_recoveries": 0, "last_source_error": None,
            "transport": {"pid": None, "process_alive": False, "stdin_open": False,
                          "binary_sha256": hashlib.sha256(args.transport.read_bytes()).hexdigest(),
                          "session_label": None, "last_consumed_tick": None, "last_event": None},
            "forwarded_frame": None, "forwarded_unix_ns": None, "forwarded_count": 0,
            "producer": None, "rejected_frame": None,
            "ledger": {"received_unix_ns": None, "producer_epoch": None, "latest_sequence": None,
                       "entries": [], "error": "waiting for authoritative ledger", "matched_to_transport": False},
            "final_stop": {"stdin_closed_unix_ns": None, "robot_verified_zero_after_close": False,
                           "record": None, "attribution": "global leash ledger; not session-specific"},
        }

    def snapshot(self):
        with self.lock:
            result = copy.deepcopy(self.data)
            alive = self.process is not None and self.process.poll() is None
            open_input = alive and self.process.stdin is not None and not self.process.stdin.closed
            matched_recently = self.last_consumed_mono is not None and time.monotonic() - self.last_consumed_mono <= 3
            result["transport"]["process_alive"] = alive
            result["transport"]["stdin_open"] = open_input
            result["transport_attached"] = bool(alive and open_input and matched_recently
                                                and result["transport"]["session_label"])
            result["published_unix_ns"] = time.time_ns()
            return result


def serve(state, port):
    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            if self.path != "/status":
                self.send_error(404)
                return
            raw = json.dumps(state.snapshot(), allow_nan=False).encode()
            try:
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(raw)))
                self.send_header("Cache-Control", "no-store")
                self.end_headers(); self.wfile.write(raw)
            except OSError:
                pass

        def log_message(self, format, *args):
            pass
    server = ThreadingHTTPServer(("127.0.0.1", port), Handler)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def read_transport(state):
    """Keep only structured allowlisted evidence, never arbitrary credential-bearing text."""
    for line in state.process.stdout:
        event = {"received_unix_ns": time.time_ns(), "kind": "diagnostic", "tick": None}
        session = re.search(r"\bsession=([0-9a-f]{8})", line)
        consumed = None
        if "qualia-leash-transport: applied T=" in line:
            event["kind"] = "applied"
            # This fixed-format line contains speeds/flags and the public label only.
            event["line"] = line.strip()
            consumed = re.search(r"applied T=(\d+)", line)
            for key in ("requested_left", "requested_right", "applied_left", "applied_right", "max_speed"):
                match = re.search(r"\b" + key + r"=(-?[0-9.]+)", line)
                event[key] = float(match.group(1)) if match else None
            event["ok"] = " ok=true " in line
            event["speed_mode"] = "low" if " speed_mode=low " in line else "unverified"
        elif "drive refused (T=" in line:
            event["kind"] = "drive_refused"
            consumed = re.search(r"drive refused \(T=(\d+)", line)
            event["reason"] = next((text for text in (
                "runtime v2 Waveshare acknowledgement timed out", "invalid pilot token", "legacy physical control is disabled",
                "drive blocked by the configured lidar collision threshold",
                "Connection refused", "timed out") if text in line), "transport reported drive refusal")
        elif "refused T=" in line:
            event["kind"] = "locally_refused"
            consumed = re.search(r"refused T=(\d+)", line)
        elif "leash acknowledged the stop:" in line:
            event["kind"] = "stop_acknowledged"
        elif "deadman expired after" in line:
            event["kind"] = "deadman_expired"
        elif "carrying our wheel commands" in line:
            event["kind"] = "started"
        else:
            continue
        with state.lock:
            if session:
                old = state.data["transport"]["session_label"]
                if old is not None and old != session.group(1):
                    state.data["error"] = "transport session changed unexpectedly"
                    state.abort.set()
                state.data["transport"]["session_label"] = session.group(1)
            if consumed:
                event["tick"] = int(consumed.group(1))
                if state.pending is not None and event["tick"] == state.pending["T"]:
                    state.pending = None
                    state.last_consumed_mono = time.monotonic()
                    state.data["transport"]["last_consumed_tick"] = event["tick"]
                    if state.data["state"] != "source_reacquiring":
                        state.data["state"] = "connected_zero_only" if state.data["zero_only"] else "connected_bounded_motion"
                else:
                    state.data["error"] = "transport consumed an unexpected producer tick"
                    state.abort.set()
            state.data["transport"]["last_event"] = event


def ledger_loop(state, base):
    while not state.stop.is_set():
        try:
            # limit=1 returns the oldest row; use its latest_sequence only as metadata.
            meta = get_json(base + "/evidence/action/applied?limit=1")
            latest = integer(meta.get("latest_sequence"), "ledger sequence", positive=False)
            page = get_json(base + "/evidence/action/applied?after_sequence=" + str(max(0, latest - 8)) + "&limit=8")
            if page.get("schema_version") != "leash.applied-action-page.v1" or len(page.get("entries", [])) > 8:
                raise ValueError("authoritative ledger contract differs")
            ledger = {"received_unix_ns": time.time_ns(), "producer_epoch": page.get("producer_epoch"),
                      "latest_sequence": page.get("latest_sequence"), "entries": page["entries"],
                      "error": None, "matched_to_transport": False}
            with state.lock:
                state.data["ledger"] = ledger
                closed = state.data["final_stop"]["stdin_closed_unix_ns"]
                if closed is not None:
                    for row in ledger["entries"]:
                        stamp = row.get("interval_end_ns")
                        if (row.get("authority") == "leash" and row.get("valid") is True
                                and type(stamp) is int and closed <= stamp <= time.time_ns() + 100000000
                                and (time.time_ns() - stamp) / 1e6 <= 1500
                                and row.get("applied_left") == 0 and row.get("applied_right") == 0
                                and type(row.get("safety_flags")) is int and row["safety_flags"] & 16):
                            state.data["final_stop"].update(robot_verified_zero_after_close=True, record=row)
        except Exception:
            with state.lock:
                state.data["ledger"]["error"] = "authoritative ledger request or contract failed"
        state.stop.wait(0.5)


def publish(state, path):
    try:
        temporary = path.with_suffix(".tmp")
        temporary.write_text(json.dumps(state.snapshot(), allow_nan=False) + "\n")
        os.replace(temporary, path)
    except OSError:
        # A reader's Windows delete-sharing lock must not kill transport supervision.
        pass


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--transport", required=True, type=Path)
    parser.add_argument("--run-dir", required=True, type=Path)
    parser.add_argument("--producer-url", default="http://10.0.0.180:8092/status")
    parser.add_argument("--base-url", default="http://10.0.0.180:8000")
    parser.add_argument("--arena", default="/qualia_observe_20260912")
    parser.add_argument("--seconds", type=int, default=60)
    parser.add_argument("--port", type=int, default=8093)
    parser.add_argument("--motion-authorized", action="store_true",
                        help="enable bounded motion after explicit operator presence and clearance approval")
    parser.add_argument("--max-speed", type=float, default=0.05,
                        help="explicit wheel cap, default 0.05 and at most 0.1 m/s")
    args = parser.parse_args()
    if not math.isfinite(args.max_speed) or not 0 < args.max_speed <= 0.1:
        parser.error("max-speed must be finite in (0, 0.1] m/s")
    if not 10 <= args.seconds <= 1800 or not 1024 <= args.port <= 65535:
        parser.error("duration or port outside bounded range")
    for url in (args.base_url, args.producer_url):
        parsed = urllib.parse.urlsplit(url)
        if parsed.scheme not in ("http", "https") or not parsed.hostname or parsed.username or parsed.password or parsed.fragment:
            parser.error("sources require HTTP(S) without URL credentials")
    if not args.transport.is_file() or args.transport.name not in ("qualia-leash-transport", "qualia-leash-transport.exe"):
        parser.error("existing qualia-leash-transport binary required")
    token_path = os.environ.get("QUALIA_LEASH_OPERATOR_TOKEN_FILE")
    if not token_path or not Path(token_path).is_file():
        parser.error("operator must provide the transport's protected token-file path")
    # Verify a live producer before launching any child that can contact the actuator.
    acquisition_deadline = time.monotonic() + 3
    while True:
        try:
            first = get_json(args.producer_url)
            permitted_frame(first, args.motion_authorized, args.max_speed)
            break
        except InterlockViolation as error:
            parser.error(str(error) + ": " + json.dumps(first.get("emitted_frame")))
        except (ValueError, KeyError, TypeError, OSError) as error:
            if time.monotonic() >= acquisition_deadline:
                parser.error("no fresh producer within 3 seconds: " + named_error(error))
            time.sleep(0.1)
    args.run_dir.mkdir(parents=True, exist_ok=False)
    state = State(args)
    server = serve(state, args.port)
    environment = os.environ.copy()
    environment.update(QUALIA_LEASH_BASE_URL=args.base_url, QUALIA_SHM_NAME=args.arena,
                       QUALIA_LEASH_TRANSPORT_ROUTE="http", QUALIA_LEASH_SPEED_MODE="low",
                       QUALIA_LEASH_TRANSPORT_DEADMAN_MS="500", QUALIA_LEASH_TRANSPORT_LEASE_TTL_SECS="20",
                       QUALIA_LEASH_TRANSPORT_HEALTH_MS="500", QUALIA_LEASH_TRANSPORT_POLL_MS="100",
                       QUALIA_LEASH_TRANSPORT_LOG_EVERY="1", QUALIA_LEASH_SENSORS_TIMEOUT_MS="2000")
    process = None
    result = 0
    phase = "transport startup"
    try:
        process = subprocess.Popen([str(args.transport.resolve())], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                   stderr=subprocess.STDOUT, text=True, encoding="utf-8", errors="replace", bufsize=1,
                                   env=environment, creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
        state.process = process
        state.data["transport"]["pid"] = process.pid
        state.data["state"] = "connecting"
        threading.Thread(target=read_transport, args=(state,), daemon=True).start()
        threading.Thread(target=ledger_loop, args=(state, args.base_url), daemon=True).start()
        deadline = time.monotonic() + args.seconds - 6
        previous_tick = None
        producer_run = first["run_id"]
        pending_since = None
        source_failure_since = None
        with (args.run_dir / "forwarded.jsonl").open("x", buffering=1) as journal:
            while time.monotonic() < deadline and not state.abort.is_set():
                if process.poll() is not None:
                    raise RuntimeError("owned transport exited")
                phase = "source acquisition"
                try:
                    value = get_json(args.producer_url)
                    frame = permitted_frame(value, args.motion_authorized, args.max_speed)
                    if source_failure_since is not None and previous_tick is not None and frame["T"] <= previous_tick:
                        raise ValueError("reacquisition requires a genuinely newer producer tick")
                    if source_failure_since is not None and time.monotonic() - source_failure_since >= 3:
                        raise RuntimeError("source reacquisition exceeded its 3-second bound")
                except InterlockViolation as error:
                    with state.lock:
                        state.data.update(state="interlock_rejected", rejected_frame=value.get("emitted_frame"),
                                          producer=value, error=str(error))
                    result = 2
                    break
                except Exception as error:
                    if source_failure_since is None:
                        source_failure_since = time.monotonic()
                    with state.lock:
                        state.data.update(state="source_reacquiring", error=named_error(error),
                                          last_source_error=named_error(error), source_failures=state.data["source_failures"] + 1)
                    publish(state, args.run_dir / "status.json")
                    if time.monotonic() - source_failure_since >= 3:
                        raise RuntimeError("source unavailable for 3 seconds: " + named_error(error))
                    # Write nothing: the existing transport's deadman owns this gap.
                    time.sleep(0.1)
                    continue
                with state.lock:
                    state.data["producer"] = value
                    if source_failure_since is not None:
                        state.data.update(source_recoveries=state.data["source_recoveries"] + 1, error=None,
                                          state="connecting")
                        source_failure_since = None
                    if value["run_id"] != producer_run or (previous_tick is not None and frame["T"] < previous_tick):
                        raise RuntimeError("producer run changed or tick moved backwards; explicit new attachment required")
                    if state.pending is not None:
                        if time.monotonic() - pending_since > 5:
                            raise RuntimeError("transport did not report the single outstanding tick within 5 seconds")
                    elif previous_tick != frame["T"]:
                        # One fresh decision only. No tail, buffered replay, or multi-frame queue.
                        phase = "final frame freshness"
                        try:
                            permitted_frame(value, args.motion_authorized, args.max_speed)
                        except InterlockViolation as error:
                            state.data.update(state="interlock_rejected", rejected_frame=frame, error=str(error))
                            result = 2
                            break
                        except (ValueError, KeyError, TypeError) as error:
                            source_failure_since = source_failure_since or time.monotonic()
                            state.data.update(state="source_reacquiring", error=named_error(error),
                                              last_source_error=named_error(error), source_failures=state.data["source_failures"] + 1)
                            publish(state, args.run_dir / "status.json")
                            continue
                        phase = "transport stdin write"
                        process.stdin.write(json.dumps(frame, separators=(",", ":"), allow_nan=False) + "\n")
                        process.stdin.flush()
                        state.pending = frame
                        pending_since = time.monotonic()
                        previous_tick = frame["T"]
                        state.data.update(forwarded_frame=frame, forwarded_unix_ns=time.time_ns(),
                                          forwarded_count=state.data["forwarded_count"] + 1)
                        phase = "forwarding evidence write"
                        journal.write(json.dumps(state.snapshot(), allow_nan=False) + "\n")
                publish(state, args.run_dir / "status.json")
                time.sleep(0.1)
    except Exception as error:
        result = 1
        with state.lock:
            state.data.update(state="source_hold", error=phase + ": " + named_error(error) + "; closing transport stdin")
    finally:
        if process is not None:
            with state.lock:
                if state.data["state"] not in ("source_hold", "interlock_rejected"):
                    state.data["state"] = "draining"
                state.data["final_stop"]["stdin_closed_unix_ns"] = time.time_ns()
                try:
                    process.stdin.close()
                except OSError:
                    state.data["error"] = "transport stdin already failed while closing"
            # Existing transport performs EOF verified stops and lease invalidation.
            drain_deadline = time.monotonic() + 5
            while process.poll() is None and time.monotonic() < drain_deadline:
                publish(state, args.run_dir / "status.json")
                time.sleep(0.1)
            process.terminate() if process.poll() is None else None
            try:
                process.wait(timeout=1)
            except subprocess.TimeoutExpired:
                process.kill(); process.wait(timeout=1)
            with state.lock:
                if state.data["state"] == "draining":
                    state.data["state"] = "stopped"
                if not state.data["final_stop"]["robot_verified_zero_after_close"]:
                    state.data["error"] = "transport ended; fresh verified-zero ledger evidence after EOF is unavailable"
                    result = 1
        state.stop.set()
        publish(state, args.run_dir / "status.json")
        server.shutdown(); server.server_close()
    return result


if __name__ == "__main__":
    raise SystemExit(main())
