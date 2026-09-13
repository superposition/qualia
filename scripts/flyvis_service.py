"""One bounded, credential-free vision or file-only shadow run for systemd."""

import argparse
import datetime
import fcntl
import json
import os
from pathlib import Path
import shutil
import signal
import socket
import subprocess
import sys
import time
import uuid


OWNER = "qualia.read-only-observation-service.v1"


def owned_runs(directory, role):
    result = []
    for candidate in directory.glob(role + "-*"):
        if candidate.is_symlink() or not candidate.is_dir():
            continue
        if candidate.resolve().parent != directory.resolve():
            continue
        marker = candidate / "service-owner.json"
        try:
            value = json.loads(marker.read_text())
        except (OSError, ValueError):
            continue
        if value.get("owner") == OWNER and value.get("role") == role and value.get("completed") is True:
            result.append(candidate)
    return sorted(result, key=lambda path: path.name, reverse=True)


def write_status(path, value):
    temporary = path.with_suffix(".tmp")
    temporary.write_text(json.dumps(value, indent=2) + "\n")
    os.replace(temporary, path)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--role", choices=("vision", "shadow"), required=True)
    parser.add_argument("--keep-completed", type=int, default=2)
    parser.add_argument("--reverse-escape", action="store_true")
    args = parser.parse_args()
    root = args.root.resolve(strict=True)
    if not 1 <= args.keep_completed <= 8:
        parser.error("keep-completed must be in 1..8")
    if args.reverse_escape and args.role != "shadow":
        parser.error("reverse-escape belongs only to the file-only shadow role")
    run_root = root / "service-runs"
    run_root.mkdir(mode=0o700, exist_ok=True)
    if run_root.is_symlink() or run_root.resolve().parent != root:
        raise ValueError("service-runs must be an owned direct child of root")
    lock = (root / ("service-" + args.role + ".lock")).open("a")
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        raise SystemExit("refusing a second service launcher for this role")
    script_name, port = ("flyvis_live_observer.py", 8091) if args.role == "vision" else ("wheel_shadow.py", 8092)
    script = root / "scripts" / script_name
    if not script.is_file():
        raise ValueError("existing observer script is absent")
    for process in Path("/proc").iterdir():
        if not process.name.isdigit():
            continue
        try:
            command = (process / "cmdline").read_bytes().decode().split("\0")
        except (OSError, UnicodeError):
            continue
        if str(script) in command:
            raise SystemExit("refusing an existing observer process; retire its owned run first")
    with socket.socket() as connection:
        connection.settimeout(0.5)
        if connection.connect_ex(("127.0.0.1", port)) == 0:
            raise SystemExit("refusing an occupied observer port")
    # The launcher only removes directories carrying its own completed marker.
    # Existing manual runs, models, images, and other services are never pruned.
    for obsolete in owned_runs(run_root, args.role)[args.keep_completed:]:
        shutil.rmtree(obsolete)
    suffix = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ") + "-" + uuid.uuid4().hex[:8]
    run = run_root / (args.role + "-" + suffix)
    metadata = {"owner": OWNER, "role": args.role, "run_directory": str(run),
                "started_unix_ns": time.time_ns(), "completed": False, "exit_code": None,
                "port": port, "interval_seconds": 1800, "transport_attached_by_service": False,
                "credentials_forwarded": False, "launcher_pid": os.getpid(), "supervisor_pid": None}
    metadata["reverse_escape_enabled"] = args.reverse_escape
    status_path = root / ("service-" + args.role + ".json")
    command = ["/usr/bin/python3", str(script), "--run-dir", str(run), "--seconds", "1800",
               "--period-ms", "200", "--port", str(port)]
    if args.role == "vision":
        command += ["--model-dir", str(root / "model"), "--base-url", "http://127.0.0.1:8000"]
    else:
        command += ["--base-url", "http://127.0.0.1:8000", "--vision-url", "http://127.0.0.1:8091/cells?type=T4a",
                    "--max-speed", "0.05"]
        if args.reverse_escape:
            command.append("--reverse-escape")
    # Build a small environment from public constants rather than copying login
    # or operator token variables. No physical transport is launched here.
    environment = {"PATH": "/usr/local/bin:/usr/bin:/bin", "HOME": str(Path.home()), "LANG": "C.UTF-8",
                   "PYTHONUNBUFFERED": "1", "CUDA_VISIBLE_DEVICES": "", "OMP_NUM_THREADS": "2",
                   "MKL_NUM_THREADS": "2", "OPENBLAS_NUM_THREADS": "2"}
    stopping = False
    child = None

    def stop(signum, frame):
        nonlocal stopping
        stopping = True
        if child is not None and child.poll() is None:
            # The group contains only this launcher's owned bounded observer.
            try:
                os.killpg(child.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass

    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    child = subprocess.Popen(command, env=environment, stdin=subprocess.DEVNULL, start_new_session=True)
    metadata["supervisor_pid"] = child.pid
    write_status(status_path, metadata)
    print(json.dumps(metadata), flush=True)
    try:
        code = child.wait(timeout=1810)
    except subprocess.TimeoutExpired:
        os.killpg(child.pid, signal.SIGTERM)
        try:
            code = child.wait(timeout=3)
        except subprocess.TimeoutExpired:
            os.killpg(child.pid, signal.SIGKILL)
            code = child.wait(timeout=2)
    metadata.update(completed=True, completed_unix_ns=time.time_ns(), exit_code=code, stopped_by_operator=stopping)
    if run.is_dir() and not run.is_symlink() and run.resolve().parent == run_root:
        write_status(run / "service-owner.json", metadata)
    write_status(status_path, metadata)
    print(json.dumps(metadata), flush=True)
    return code


if __name__ == "__main__":
    raise SystemExit(main())
