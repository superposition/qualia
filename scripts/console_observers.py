"""Keep the console's read-only observers alive while its window is open.

No motor client or operator credential is used. Existing owned observers are
adopted; replacements retain their original bounded execution and evidence.
"""
import argparse
import ctypes
from ctypes import wintypes
import json
import msvcrt
import os
from pathlib import Path
import subprocess
import sys
import time


kernel = ctypes.WinDLL("kernel32", use_last_error=True)
kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
kernel.OpenProcess.restype = wintypes.HANDLE
kernel.QueryFullProcessImageNameW.argtypes = [wintypes.HANDLE, wintypes.DWORD, wintypes.LPWSTR, ctypes.POINTER(wintypes.DWORD)]
kernel.GetExitCodeProcess.argtypes = [wintypes.HANDLE, ctypes.POINTER(wintypes.DWORD)]
kernel.CloseHandle.argtypes = [wintypes.HANDLE]
kernel.TerminateProcess.argtypes = [wintypes.HANDLE, wintypes.UINT]


def owned_process(pid, executable, terminate=False):
    handle = kernel.OpenProcess(0x1000 | (1 if terminate else 0), False, int(pid or 0))
    if not handle:
        return False
    try:
        size = wintypes.DWORD(32768)
        name = ctypes.create_unicode_buffer(size.value)
        code = wintypes.DWORD()
        if not kernel.QueryFullProcessImageNameW(handle, 0, name, ctypes.byref(size)):
            return False
        if os.path.normcase(name.value) != os.path.normcase(str(executable.resolve())):
            return False
        if not kernel.GetExitCodeProcess(handle, ctypes.byref(code)) or code.value != 259:
            return False
        if terminate:
            kernel.TerminateProcess(handle, 0)
        return True
    finally:
        kernel.CloseHandle(handle)


def write_json(path, value):
    temporary = path.with_suffix(".tmp")
    temporary.write_text(json.dumps(value), encoding="utf-8")
    os.replace(str(temporary), str(path))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runtime", required=True, type=Path)
    args = parser.parse_args()
    runtime = args.runtime.resolve()
    lock = (runtime / "observer-supervisor.lock").open("a+b")
    lock.seek(0)
    if not lock.read(1):
        lock.write(b"1"); lock.flush()
    lock.seek(0)
    msvcrt.locking(lock.fileno(), msvcrt.LK_NBLCK, 1)
    metadata = runtime / "current-console-pids.json"
    scripts = Path(__file__).resolve().parent
    executables = {"coach": runtime / "qualia-coach-observer.exe",
                   "perception": runtime / "qualia-perception-observer.exe",
                   "pretrained_mirror": Path(sys.executable)}
    missing_console_since = None
    restart_after = {}
    while True:
        pids = json.loads(metadata.read_text(encoding="utf-8"))
        console_alive = owned_process(pids.get("console"), runtime / "qualia-console.exe")
        if console_alive:
            missing_console_since = None
        elif missing_console_since is None:
            missing_console_since = time.monotonic()
        if missing_console_since is not None and time.monotonic() - missing_console_since > 30:
            for role, executable in executables.items():
                owned_process(pids.get(role), executable, terminate=True)
            write_json(runtime / "observer-supervisor.json", {"state": "stopped", "reason": "console closed", "pid": os.getpid()})
            return
        status = {"state": "running", "pid": os.getpid(), "observed_ms": int(time.time() * 1000), "roles": {}}
        for role, executable in executables.items():
            if owned_process(pids.get(role), executable):
                status["roles"][role] = {"pid": pids[role], "process_running": True}
                continue
            if not console_alive or time.monotonic() < restart_after.get(role, 0):
                status["roles"][role] = {"process_running": False, "state": "waiting to restart"}
                continue
            restart_after[role] = time.monotonic() + 15
            env = os.environ.copy()
            for key in ("DEEPSEEK_API_KEY", "QUALIA_COACH_API_KEY", "QUALIA_CAMERA_AIM_TOKEN", "QUALIA_LEASH_OPERATOR_TOKEN_FILE"):
                env.pop(key, None)
            stamp = str(time.time_ns())
            try:
                if role == "coach":
                    credential = subprocess.run([str(Path.home() / ".bun/bin/omp.exe"), "token", "deepseek"],
                                                capture_output=True, text=True, timeout=15, check=True).stdout.strip()
                    if not credential or any(c.isspace() for c in credential):
                        raise ValueError("credential unavailable")
                    env.update(DEEPSEEK_API_KEY=credential, QUALIA_COACH_SOURCE_URL="http://10.0.0.180:8091/status",
                               QUALIA_COACH_TIMEOUT_MS="15000", QUALIA_COACH_MODEL="deepseek-chat", QUALIA_COACH_BASE_URL="https://api.deepseek.com")
                    command = [str(executable), "--status", str(runtime / "pretrained-status.json"), "--evidence",
                               str(runtime / ("coach-supervised-" + stamp + ".jsonl")), "--ticks", "60", "--poll-ms", "20000"]
                elif role == "perception":
                    env.update(QUALIA_SHM_NAME="/qualia_observe_20260912", QUALIA_OBSERVE_SECONDS="1800",
                               QUALIA_CAMERA_SNAPSHOT_URL="http://10.0.0.180:8000/camera/snapshot",
                               QUALIA_PERCEPTION_STATUS=str(runtime / "perception-status.json"),
                               QUALIA_PERCEPTION_EVIDENCE=str(runtime / ("perception-supervised-" + stamp + ".jsonl")))
                    command = [str(executable)]
                else:
                    command = [str(executable), str(scripts / "mirror_remote_fly.py"), "--url", "http://10.0.0.180:8091/status",
                               "--output", str(runtime / "pretrained-status.json"), "--duration", "1800"]
                with (runtime / (role + "-supervised-" + stamp + ".log")).open("wb") as log:
                    process = subprocess.Popen(command, env=env, stdout=log, stderr=subprocess.STDOUT, creationflags=subprocess.CREATE_NO_WINDOW)
                pids[role] = process.pid
                write_json(metadata, pids)
                status["roles"][role] = {"pid": process.pid, "process_running": True, "state": "restarted"}
            except Exception:
                status["roles"][role] = {"process_running": False, "state": "restart failed; retrying in 15 seconds"}
        write_json(runtime / "observer-supervisor.json", status)
        time.sleep(3)


if __name__ == "__main__":
    main()
