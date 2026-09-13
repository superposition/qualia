"""Bounded zero-only attachment of fresh wheel decisions to the existing transport.

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
import urllib.request
import uuid

from wheel_shadow import NoRedirect, integer, number


FRESH_MS = 500


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
            "zero_only": True, "nonzero_enabled": False, "transport_attached": False, "error": None,
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
                    state.data["state"] = "connected_zero_only"
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
    args = parser.parse_args()
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
    first = get_json(args.producer_url)
    first_frame = current_frame(first)
    if first_frame["L"] != 0 or first_frame["R"] != 0:
        parser.error("zero-only interlock refuses before starting transport: " + json.dumps(first_frame))
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
        with (args.run_dir / "forwarded.jsonl").open("x", buffering=1) as journal:
            while time.monotonic() < deadline and not state.abort.is_set():
                if process.poll() is not None:
                    raise RuntimeError("owned transport exited")
                value = get_json(args.producer_url)
                frame = current_frame(value)
                with state.lock:
                    state.data["producer"] = value
                    if frame["L"] != 0 or frame["R"] != 0:
                        state.data.update(state="interlock_rejected", rejected_frame=frame,
                                          error="zero-only interlock rejected a nonzero frame; no substitution")
                        result = 2
                        break
                    if value["run_id"] != producer_run or (previous_tick is not None and frame["T"] < previous_tick):
                        raise RuntimeError("producer run changed or tick moved backwards; explicit new attachment required")
                    if state.pending is not None:
                        if time.monotonic() - pending_since > 5:
                            raise RuntimeError("transport did not report the single outstanding tick within 5 seconds")
                    elif previous_tick != frame["T"]:
                        # One fresh decision only. No tail, buffered replay, or multi-frame queue.
                        current_frame(value)
                        process.stdin.write(json.dumps(frame, separators=(",", ":"), allow_nan=False) + "\n")
                        process.stdin.flush()
                        state.pending = frame
                        pending_since = time.monotonic()
                        previous_tick = frame["T"]
                        state.data.update(forwarded_frame=frame, forwarded_unix_ns=time.time_ns(),
                                          forwarded_count=state.data["forwarded_count"] + 1)
                        journal.write(json.dumps(state.snapshot(), allow_nan=False) + "\n")
                publish(state, args.run_dir / "status.json")
                time.sleep(0.1)
    except Exception:
        result = 1
        with state.lock:
            state.data.update(state="source_hold", error="source, transport, or evidence failed; closing transport stdin")
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
