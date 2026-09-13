"""Bounded read-only camera observer using a frozen exported visual network."""

from __future__ import annotations

import argparse
import copy
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import math
import os
from pathlib import Path
import subprocess
import sys
import threading
import time
import urllib.parse
import urllib.request
import uuid


FRESH_MS = 1500
PRODUCER_HEADERS = {
    "X-Leash-Camera-Sequence": "sequence",
    "X-Leash-Camera-Dequeued-Ms": "dequeued_unix_ms",
    "X-Leash-Camera-Acquisition-Started-Ms": "acquisition_started_unix_ms",
    "X-Leash-Camera-Driver-Timestamp-Us": "driver_timestamp_us",
    "X-Leash-Camera-Driver-Timestamp-Flags": "driver_timestamp_flags",
    "X-Leash-Camera-Timestamp-Basis": "timestamp_basis",
}


class RejectRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise ValueError("read-only observer refuses HTTP redirects")


def read_request(url, limit, timeout):
    request = urllib.request.Request(url, headers={"Cache-Control": "no-cache"})
    with urllib.request.build_opener(RejectRedirect).open(request, timeout=timeout) as response:
        if response.status != 200:
            raise ValueError(f"HTTP {response.status}")
        raw, headers = response.read(limit + 1), response.headers
    if len(raw) > limit:
        raise ValueError("response exceeds byte budget")
    return raw, headers


def camera_input(url):
    started_ns = time.time_ns()
    started_mono = time.monotonic()
    raw, headers = read_request(url, 8 * 1024 * 1024, 2)
    received_mono, received_ns = time.monotonic(), time.time_ns()
    if headers.get_content_type() != "image/jpeg":
        raise ValueError("camera response is not image/jpeg")
    producer = {field: headers[name] for name, field in PRODUCER_HEADERS.items() if headers.get(name) is not None}
    for field in ("sequence", "dequeued_unix_ms", "acquisition_started_unix_ms", "driver_timestamp_us", "driver_timestamp_flags"):
        if field in producer:
            try:
                producer[field] = int(producer[field])
            except ValueError:
                raise ValueError(f"invalid camera producer {field}")
    acquisition_ms = producer.get("acquisition_started_unix_ms")
    if acquisition_ms is not None and not -100 <= received_ns / 1e6 - acquisition_ms <= FRESH_MS:
        raise ValueError("camera producer acquisition timestamp is stale or ahead of robot clock")
    return raw, {
        "url": url, "request_started_unix_ns": started_ns, "received_unix_ns": received_ns,
        "request_elapsed_ms": (received_mono - started_mono) * 1000,
        "camera_exposure_unix_ns": None, "jpeg_sha256": hashlib.sha256(raw).hexdigest(),
        "producer": producer, "_received_mono": received_mono,
    }


class SharedStatus:
    def __init__(self, args):
        self.lock = threading.Lock()
        self.stop = threading.Event()
        self.latest_input = None
        self.started = time.monotonic()
        started_ns = time.time_ns()
        self.data = {
            "schema": "qualia.flyvis-live.v1", "run_id": args.run_id, "state": "starting",
            "started_unix_ns": started_ns, "deadline_unix_ns": started_ns + int(args.seconds * 1e9),
            "published_unix_ns": started_ns, "tick": 0, "camera_frames": 0,
            "activity_kind": "graded_model_voltage", "parameters_frozen": True, "controls_locked": True,
            "identity_space": "flyvis model indices; not MaleCNS body IDs",
            "model_inputs": ["camera_luminance"], "measured_context_drives_model": False,
            "source": {"url": args.base_url + "/camera/snapshot", "request_started_unix_ns": None,
                       "received_unix_ns": None, "request_elapsed_ms": None, "camera_exposure_unix_ns": None,
                       "jpeg_sha256": None, "producer": {}, "_received_mono": None},
            "latest_camera": None, "output": None, "error": None,
            "compute_ms": None, "memory_rss_mib": None, "memory_available_mib": None, "threads": 2,
            "model": json.loads((args.model_dir / "model.json").read_text()), "measured_context": {},
            "warmup": {"steps": 50, "dt_s": 0.02, "input": "first real image held; not new camera acquisitions"},
            "time_semantics": "causal zero-order hold between host receipt times; model time excludes warmup",
        }

    def update(self, **fields):
        with self.lock:
            self.data.update(fields)

    def snapshot(self):
        with self.lock:
            result = copy.deepcopy(self.data)
        now = time.monotonic()
        result["published_unix_ns"] = time.time_ns()
        for name in ("source", "latest_camera"):
            if result[name] is not None:
                stamp = result[name].pop("_received_mono")
                result[name]["received_age_ms"] = None if stamp is None else max(0, (now - stamp) * 1000)
        output = result["output"]
        if output is not None:
            output["age_ms"] = max(0, (now - output.pop("_completed_mono")) * 1000)
            output["input_age_ms"] = max(0, (now - output.pop("_input_mono")) * 1000)
        for bundle in result["measured_context"].values():
            stamp = bundle.pop("_received_mono", None)
            bundle["age_ms"] = None if stamp is None else max(0, (now - stamp) * 1000)
        if result["state"] == "running" and (output is None or result["source"] is None
                or output["age_ms"] > FRESH_MS or output["input_age_ms"] > FRESH_MS
                or result["source"]["received_age_ms"] > FRESH_MS):
            result["state"] = "stale"
            result["error"] = "graded response or actual source image exceeds 1500 ms freshness window"
        return result


def serve_status(shared, port):
    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            if self.path == "/status":
                raw = json.dumps(shared.snapshot(), allow_nan=False).encode()
                content_type = "application/json"
            elif self.path == "/input.jpg":
                with shared.lock:
                    raw = shared.latest_input
                if raw is None:
                    self.send_error(503, "No current model input")
                    return
                content_type = "image/jpeg"
            else:
                self.send_error(404)
                return
            try:
                self.send_response(200)
                self.send_header("Content-Type", content_type)
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
    threading.Thread(target=server.serve_forever, daemon=True, name="flyvis-status").start()
    return server


def publish_file(shared, path):
    while not shared.stop.is_set():
        try:
            temporary = path.with_suffix(".tmp")
            temporary.write_text(json.dumps(shared.snapshot(), allow_nan=False) + "\n")
            os.replace(temporary, path)
        except OSError as error:
            print(f"status-file warning: {error}", flush=True)
        shared.stop.wait(0.5)


def observe_context(shared, base_url):
    endpoints = {"telemetry": "/telemetry/compact", "camera_lights": "/camera/lights"}
    while not shared.stop.is_set():
        for name, path in endpoints.items():
            if shared.stop.is_set():
                break
            try:
                raw, _ = read_request(base_url + path, 256 * 1024, 1)
                measured = json.loads(raw)
                json.dumps(measured, allow_nan=False)
                bundle = {"received_unix_ns": time.time_ns(), "_received_mono": time.monotonic(),
                          "data": measured, "error": None}
            except Exception as error:
                bundle = {"received_unix_ns": None, "data": None, "error": str(error)}
            with shared.lock:
                shared.data["measured_context"][name] = bundle
        shared.stop.wait(2)


def worker(args):
    os.environ.update(CUDA_VISIBLE_DEVICES="", OMP_NUM_THREADS="2", MKL_NUM_THREADS="2", OPENBLAS_NUM_THREADS="2")
    shared = SharedStatus(args)
    server = serve_status(shared, args.port)
    threading.Thread(target=publish_file, args=(shared, args.run_dir / "status.json"), daemon=True).start()
    threading.Thread(target=observe_context, args=(shared, args.base_url), daemon=True).start()
    try:
        import psutil
        import torch
        torch.set_num_threads(2)
        torch.set_num_interop_threads(2)
        torch.set_default_device("cpu")
        from flyvis_frozen import FrozenVisualModel, DT

        if psutil.virtual_memory().available < 700 * 2**20:
            raise MemoryError("less than 700 MiB available before loading visual model")
        model = FrozenVisualModel(args.model_dir)
        if model.metadata.get("status") != "complete" or not model.metadata.get("parity", {}).get("parameters_unchanged"):
            raise ValueError("export has no completed frozen reference-parity evidence")
        process = psutil.Process()
        previous_input, previous_record, previous_jpeg = None, None, None
        previous_boundary = None
        model_time, residual, frames, tick = 0.0, 0.0, 0, 0
        with torch.inference_mode(), (args.run_dir / "activity.jsonl").open("a", buffering=1) as journal:
            while time.monotonic() - shared.started < args.seconds - 3:
                available, rss = psutil.virtual_memory().available, process.memory_info().rss
                shared.update(memory_rss_mib=rss / 2**20, memory_available_mib=available / 2**20)
                if available < 350 * 2**20 or rss > 1100 * 2**20:
                    shared.update(state="memory_hold", error="memory guard paused model", output=None)
                    previous_input = previous_record = previous_jpeg = None
                    shared.stop.wait(1)
                    continue
                began = time.monotonic()
                try:
                    jpeg, record = camera_input(args.base_url + "/camera/snapshot")
                    frames += 1
                    shared.update(camera_frames=frames, latest_camera=record)
                    if previous_record is not None:
                        old_sequence = previous_record.get("producer", {}).get("sequence")
                        new_sequence = record.get("producer", {}).get("sequence")
                        if old_sequence is not None and old_sequence == new_sequence:
                            shared.update(state="waiting_for_camera", error="camera producer sequence did not advance", output=None)
                            shared.stop.wait(0.2)
                            continue
                    retina = model.retina(jpeg)
                    gap = None if previous_boundary is None else record["_received_mono"] - previous_boundary
                    if previous_input is None or gap is None or gap <= 0 or gap * 1000 > FRESH_MS:
                        shared.update(state="warming_up", source=record, output=None, error=None)
                        model.reset()
                        warm_start = time.monotonic()
                        model.advance(retina, 50)
                        shared.update(compute_ms=(time.monotonic() - warm_start) * 1000)
                        model_time, residual = 0.0, 0.0
                        previous_input, previous_record, previous_jpeg = retina, record, jpeg
                        previous_boundary = time.monotonic()
                        shared.stop.wait(max(0, args.period_ms / 1000 - (time.monotonic() - began)))
                        continue
                    residual += gap
                    steps = int(residual / DT)
                    if steps < 1:
                        shared.stop.wait(DT)
                        continue
                    if steps > 75:
                        raise ValueError("integration continuity exceeded 1500 ms budget")
                    residual -= steps * DT
                    previous_activity = model.activity.numpy().copy()
                    compute_started = time.monotonic()
                    model.advance(previous_input, steps)
                    compute_ms = (time.monotonic() - compute_started) * 1000
                    model_time += steps * DT
                    output = model.statistics(previous_activity)
                    output.update(completed_unix_ns=time.time_ns(), _completed_mono=time.monotonic(),
                                  model_time_s=model_time, input_received_unix_ns=previous_record["received_unix_ns"],
                                  input_sha256=previous_record["jpeg_sha256"], _input_mono=previous_record["_received_mono"])
                    tick += 1
                    with shared.lock:
                        shared.latest_input = previous_jpeg
                        shared.data.update(state="running", source=previous_record, output=output, tick=tick,
                                           compute_ms=compute_ms, error=None)
                    snapshot = shared.snapshot()
                    journal.write(json.dumps(snapshot, allow_nan=False) + "\n")
                    if tick == 1 or tick % 20 == 0:
                        print(f"run={args.run_id} tick={tick} state={snapshot['state']} compute_ms={compute_ms:.1f} voltage=[{output['voltage_min']:.6f},{output['voltage_max']:.6f}] rss_mib={rss / 2**20:.1f}", flush=True)
                    previous_input, previous_record, previous_jpeg = retina, record, jpeg
                    previous_boundary = record["_received_mono"]
                except Exception as error:
                    shared.update(state="waiting_for_camera", error=str(error), output=None)
                    previous_input = previous_record = previous_jpeg = None
                    shared.stop.wait(0.5)
                shared.stop.wait(max(0, args.period_ms / 1000 - (time.monotonic() - began)))
        shared.update(state="stopped", error="bounded observation interval ended")
        return 0
    except Exception as error:
        shared.update(state="failed", error=str(error), output=None)
        print(f"observer failed: {error}", file=sys.stderr, flush=True)
        return 1
    finally:
        (args.run_dir / "status.json").write_text(json.dumps(shared.snapshot(), allow_nan=False) + "\n")
        shared.stop.set()
        server.shutdown()
        server.server_close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model-dir", required=True, type=Path)
    parser.add_argument("--run-dir", required=True, type=Path)
    parser.add_argument("--base-url", default="http://127.0.0.1:8000")
    parser.add_argument("--port", type=int, default=8091)
    parser.add_argument("--seconds", type=int, default=1800)
    parser.add_argument("--period-ms", type=int, default=200)
    parser.add_argument("--run-id", default=None)
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    base = urllib.parse.urlsplit(args.base_url)
    if base.scheme not in ("http", "https") or not base.hostname or base.username or base.password or base.query or base.fragment or base.path not in ("", "/"):
        parser.error("base-url must be an HTTP origin without credentials")
    args.base_url = args.base_url.rstrip("/")
    if not 10 <= args.seconds <= 1800 or not 100 <= args.period_ms <= 1000 or not 1024 <= args.port <= 65535:
        parser.error("seconds10..1800, period-ms100..1000, port1024..65535 required")
    if args.worker:
        return worker(args)
    args.run_dir.mkdir(parents=True, exist_ok=False)
    run_id = args.run_id or str(uuid.uuid4())
    command = [sys.executable, str(Path(__file__).resolve()), *sys.argv[1:], "--worker", "--run-id", run_id]
    try:
        result = subprocess.run(command, timeout=args.seconds,
                                creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
        return result.returncode
    except subprocess.TimeoutExpired:
        (args.run_dir / "status.json").write_text(json.dumps({"schema": "qualia.flyvis-live.v1", "run_id": run_id,
                                                            "state": "stopped", "error": "supervisor wall-time bound ended"}) + "\n")
        return 0


if __name__ == "__main__":
    raise SystemExit(main())
