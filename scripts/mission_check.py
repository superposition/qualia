#!/usr/bin/env python3
"""Step 29's two assertions, as one checker (ticket #45, EPIC-10 #15).

Step 29 is the board half of the end-to-end run: "Rebuild the fatbin per Step 18
(`CUDAARCHS=87-real`), copy the prior directory, run the same zero-motion stack
with `QUALIA_CUDA_SM=87`, and assert `nvidia-smi` reports under 8 GB used and
that `GET /braid` reaches the same terminal state."

Two assertions, then:

  braid-terminal  one bounded exploration mission is delivered to the agent's
                  broker and then cancelled — the two envelopes Step 28's run
                  sends — `GET /braid`'s `open_missions` rises by one, and the
                  count returns to where it started with the mission record
                  terminal.
  gpu-memory      the peak memory the run used stays under the 8 GB bound.
                  `nvidia-smi` is asked first; a Jetson reports `[N/A]` for its
                  memory counters, so `tegrastats` (the board's own sampler) is
                  the documented second source, and when neither produces a
                  number the leg reports that it cannot be judged rather than
                  passing on an unread value.

The transitions and the envelope are pure functions, so `--self-test` exercises
them here, on the dev host, with no board and no stack: the boundary cases of the
1 -> 0 transition, the 8192 MiB bound, both memory parsers, and the agent's
`MissionEnvelopeV1::validate()` bounds (`crates/sync-types/src/mission.rs`), which
a generated envelope must satisfy or the broker refuses it with `400`.

Usage:

    python3 scripts/mission_check.py --agent-url https://127.0.0.1:8081 \\
        --token "$QUALIA_MISSION_BROKER_TOKEN"
    python3 scripts/mission_check.py --self-test

The token is the mission-broker bearer (`QUALIA_MISSION_BROKER_TOKEN`, or
`--token`): the agent never trusts loopback for mission intake, so without one no
mission can be delivered and the leg reports that instead of guessing.

Exit codes: 0 when both assertions hold and `mission: OK` is printed; 1 when an
assertion fails; 2 when a leg cannot be judged (no number from either memory
source, no broker token, no agent) — an unjudged board run is not a pass.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import ssl
import subprocess
import sys
import time
import urllib.error
import urllib.request

BRAID_PATH = "/braid"
MISSIONS_PATH = "/mission-control/missions"
ENVELOPES_PATH = "/mission-control/envelopes"

# `crates/sync-types/src/mission.rs` fixes these; the envelope this checker builds
# has to satisfy the same `validate()` the broker runs, or the delivery is a 400.
MISSION_ENVELOPE_SCHEMA = "qualia.mission-envelope.v1"
MAX_DEADLINE_MS = 120_000
MIN_RUNTIME_MS = 100
MAX_RUNTIME_MS = 120_000
MIN_SPEED_MPS = 0.01
MAX_SPEED_MPS = 0.25
MIN_DISTANCE_M = 0.05
MAX_DISTANCE_M = 5.0
MAX_REPLANS = 8
MIN_EVIDENCE_MAX_AGE_MS = 100
MAX_EVIDENCE_MAX_AGE_MS = 5_000
MAX_AREA_SPAN_M = 20.0

# Step 29's bound: "under 8 GB used".
DEFAULT_BUDGET_MIB = 8_192
# The mission's own bound; the broker closes it at the deadline with
# `deadline_exceeded`, which is the deterministic end of the transition.
DEFAULT_DEADLINE_S = 20
DEFAULT_OPEN_TIMEOUT_S = 60.0
DEFAULT_CLOSE_TIMEOUT_S = 120.0
# The mission is held open this long before the cancel, so the memory sampler
# sees the stack working rather than one instant of it.
DEFAULT_HOLD_S = 3.0
POLL_INTERVAL_S = 0.5
MEMORY_SAMPLE_INTERVAL_S = 1.0

TERMINAL_STATUSES = ("completed", "failed", "cancelled")


# --------------------------------------------------------------------------
# The transition, as a pure function
# --------------------------------------------------------------------------

def evaluate_transition(samples, before, open_timeout_s, close_timeout_s):
    """Judge the `open_missions` 1 -> 0 transition from the observed samples.

    `samples` is a list of `(elapsed_s, open_missions)`. Returns
    `(verdict, detail)` with verdict `"OK"`, `"FAIL"` or `"CANNOT-ASSERT"`.
    A mission that opened but never closed inside the close window fails, and so
    does one that never opened.
    """
    if not samples:
        return "CANNOT-ASSERT", "the braid was never read"
    counts = [count for _, count in samples]
    opened = next((elapsed for elapsed, count in samples if count > before), None)
    if opened is None:
        return "FAIL", "open_missions never rose above %d (saw %s over %.1f s)" % (
            before, _sequence(counts), samples[-1][0])
    if opened > open_timeout_s:
        return "FAIL", "open_missions took %.1f s to rise above %d (limit %.1f s)" % (
            opened, before, open_timeout_s)
    closed = next(
        (elapsed for elapsed, count in samples if elapsed >= opened and count <= before),
        None,
    )
    if closed is None:
        return "FAIL", "open_missions never returned to %d after opening above it (saw %s over %.1f s)" % (
            before, _sequence(counts), samples[-1][0])
    if closed - opened > close_timeout_s:
        return "FAIL", "open_missions took %.1f s to return to %d (limit %.1f s)" % (
            closed - opened, before, close_timeout_s)
    return "OK", "open_missions %s (started at %d; rose %.1f s in, returned %.1f s in)" % (
        _sequence(counts), before, opened, closed)


def _sequence(counts):
    """The counts with runs collapsed, so a failure detail line stays readable."""
    distinct = []
    for count in counts:
        if not distinct or distinct[-1] != count:
            distinct.append(count)
    return " -> ".join(str(count) for count in distinct)


def evaluate_memory(peak_mib, budget_mib):
    """Judge the memory bound. Step 29 says *under* 8 GB, so the bound is strict."""
    if peak_mib is None:
        return "CANNOT-ASSERT", "neither nvidia-smi nor tegrastats produced a number"
    if peak_mib < budget_mib:
        return "OK", "peak %d MiB of %d MiB budget" % (peak_mib, budget_mib)
    return "FAIL", "peak %d MiB is not under the %d MiB budget" % (peak_mib, budget_mib)


# --------------------------------------------------------------------------
# The envelope, as a pure function
# --------------------------------------------------------------------------

def build_envelope(mission_id, issued_at_ms, deadline_s, idempotency_key=None):
    """A bounded `explore_frontier` envelope the broker's `validate()` accepts.

    The deadline is clamped into the documented window: the broker rejects an
    envelope whose deadline is more than 120 s after issue, and `ingest` rejects
    one already past, so `deadline_s` is clamped to 1..=120.
    """
    deadline_s = max(1, min(int(deadline_s), MAX_DEADLINE_MS // 1000))
    runtime_ms = max(MIN_RUNTIME_MS, min(MAX_RUNTIME_MS, deadline_s * 1000))
    return {
        "schema_version": MISSION_ENVELOPE_SCHEMA,
        "broker_id": "board-t29",
        "producer_epoch": 1,
        "sequence": 1,
        "mission_id": mission_id,
        "idempotency_key": idempotency_key or mission_id,
        "command": "start",
        "issued_at_ms": issued_at_ms,
        "deadline_ms": issued_at_ms + deadline_s * 1000,
        "objective": {
            "kind": "explore_frontier",
            "summary": "board end-to-end: bounded frontier sweep (T29, step 29)",
            "target_x_m": None,
            "target_y_m": None,
            "tolerance_m": None,
        },
        "constraints": {
            "operating_area": {
                "frame_id": "odom",
                "min_x_m": -10.0,
                "min_y_m": -10.0,
                "max_x_m": 10.0,
                "max_y_m": 10.0,
            },
            "speed_ceiling_mps": 0.05,
            "max_distance_m": 1.0,
            "max_runtime_ms": runtime_ms,
            "max_replans": 2,
            "evidence_max_age_ms": 1_000,
        },
        # A `start` with no evidence reference is refused by `validate()`.
        "evidence_refs": [
            "docs/decisions.md#d-010--the-deploy-target-pinkie-is-attached-to-the-dev-host"
        ],
        "fly_governed": True,
    }


def build_cancel_envelope(mission_id, issued_at_ms, deadline_s):
    """The second envelope of the pair: same mission, `cancel`, sequence 2.

    Step 28's run opens a mission and then ends it; the broker closes a cancelled
    mission with `stop_verified` because it never entered motion. The start
    envelope's deadline stays in place as the backstop, so a lost cancel still
    ends the mission.
    """
    envelope = build_envelope(
        mission_id, issued_at_ms, deadline_s, idempotency_key="%s-cancel" % mission_id
    )
    envelope["sequence"] = 2
    envelope["command"] = "cancel"
    return envelope


def envelope_bounds_errors(envelope):
    """The broker's `validate()` bounds, re-stated as a list of violations.

    This is the checker's own reading of `crates/sync-types/src/mission.rs`; the
    self-test runs it over the generated envelope, so a drift here or a change to
    the generator fails on the host, before a board run does.
    """
    errors = []
    issued, deadline = envelope["issued_at_ms"], envelope["deadline_ms"]
    if deadline <= issued or deadline - issued > MAX_DEADLINE_MS:
        errors.append("deadline must be within %d ms of issue" % MAX_DEADLINE_MS)
    objective = envelope["objective"]
    if not objective["summary"].strip() or len(objective["summary"]) > 512:
        errors.append("objective summary must be 1..=512 characters")
    area = envelope["constraints"]["operating_area"]
    if area["frame_id"] != "odom":
        errors.append("operating area frame must be odom")
    if area["max_x_m"] - area["min_x_m"] > MAX_AREA_SPAN_M:
        errors.append("operating area is wider than %.0f m" % MAX_AREA_SPAN_M)
    if area["max_y_m"] - area["min_y_m"] > MAX_AREA_SPAN_M:
        errors.append("operating area is deeper than %.0f m" % MAX_AREA_SPAN_M)
    constraints = envelope["constraints"]
    if not MIN_SPEED_MPS <= constraints["speed_ceiling_mps"] <= MAX_SPEED_MPS:
        errors.append("speed ceiling is outside the bounded low-speed limits")
    if not MIN_DISTANCE_M <= constraints["max_distance_m"] <= MAX_DISTANCE_M:
        errors.append("max distance is outside the bounded limits")
    if not MIN_RUNTIME_MS <= constraints["max_runtime_ms"] <= MAX_RUNTIME_MS:
        errors.append("max runtime is outside the bounded limits")
    if constraints["max_replans"] > MAX_REPLANS:
        errors.append("replan allowance exceeds %d" % MAX_REPLANS)
    if not MIN_EVIDENCE_MAX_AGE_MS <= constraints["evidence_max_age_ms"] <= MAX_EVIDENCE_MAX_AGE_MS:
        errors.append("evidence max age is outside the bounded limits")
    if envelope["command"] == "start" and not envelope["evidence_refs"]:
        errors.append("a start requires at least one evidence reference")
    if envelope["schema_version"] != MISSION_ENVELOPE_SCHEMA:
        errors.append("unexpected envelope schema")
    if int(envelope["producer_epoch"]) <= 0 or int(envelope["sequence"]) <= 0:
        errors.append("producer epoch and sequence must be non-zero")
    return errors


# --------------------------------------------------------------------------
# The two memory sources
# --------------------------------------------------------------------------

def parse_nvidia_smi_used_mib(text):
    """`nvidia-smi` memory.used in MiB, or None when it is not a number.

    A Jetson reports `[N/A]` here, which is a reading that cannot be judged — not
    a passing zero.
    """
    for line in text.splitlines():
        line = line.strip()
        if not line:
            continue
        match = re.match(r"^(\d+)(?:\s|$)", line)
        return int(match.group(1)) if match else None
    return None


def parse_tegrastats_used_mib(text):
    """The `RAM <used>/<total>MB` field of a `tegrastats` line, or None."""
    match = re.search(r"RAM (\d+)/(\d+)MB", text)
    return int(match.group(1)) if match else None


def sample_nvidia_smi():
    """`(mib, source, note)` from `nvidia-smi`, keeping the raw reading on failure."""
    try:
        done = subprocess.run(
            ["nvidia-smi", "--query-gpu=memory.used", "--format=csv,noheader,nounits"],
            capture_output=True,
            text=True,
            timeout=10,
        )
    except (OSError, subprocess.SubprocessError) as error:
        return None, "nvidia-smi", "nvidia-smi did not run: %s" % error
    raw = (done.stdout or "").strip()
    if done.returncode != 0:
        return None, "nvidia-smi", "nvidia-smi exit %d: %s" % (
            done.returncode,
            (done.stderr or "").strip() or "no output",
        )
    value = parse_nvidia_smi_used_mib(raw)
    if value is None:
        return None, "nvidia-smi", "nvidia-smi reported %r" % raw
    return value, "nvidia-smi", ""


def sample_tegrastats():
    """`(mib, source, note)` from `tegrastats`, the board's own sampler."""
    try:
        done = subprocess.run(
            ["timeout", "3", "tegrastats", "--interval", "500"],
            capture_output=True,
            text=True,
            timeout=15,
        )
    except (OSError, subprocess.SubprocessError) as error:
        return None, "tegrastats", "tegrastats did not run: %s" % error
    if done.returncode not in (0, 124):
        return None, "tegrastats", "tegrastats exit %d: %s" % (
            done.returncode,
            (done.stderr or "").strip() or "no output",
        )
    value = parse_tegrastats_used_mib(done.stdout or "")
    if value is None:
        return None, "tegrastats", "tegrastats printed no RAM line"
    return value, "tegrastats", ""


def sample_memory(source):
    """One reading from the requested source, or the documented fallback chain."""
    if source == "nvidia-smi":
        return sample_nvidia_smi()
    if source == "tegrastats":
        return sample_tegrastats()
    mib, origin, note = sample_nvidia_smi()
    if mib is not None:
        return mib, origin, note
    fallback_mib, fallback_origin, fallback_note = sample_tegrastats()
    if fallback_mib is not None:
        return fallback_mib, fallback_origin, "nvidia-smi unusable (%s); tegrastats supplied the reading" % note
    return None, "auto", "%s; %s" % (note, fallback_note)


# --------------------------------------------------------------------------
# HTTP
# --------------------------------------------------------------------------

def _request(url, payload=None, token=None, timeout=10):
    """One bounded exchange. Returns `(status, decoded_body)`."""
    headers = {"Accept": "application/json"}
    data = None
    if payload is not None:
        data = json.dumps(payload).encode()
        headers["Content-Type"] = "application/json"
    if token:
        headers["Authorization"] = "Bearer " + token
    request = urllib.request.Request(url, data=data, headers=headers)
    context = ssl._create_unverified_context() if url.startswith("https") else None
    with urllib.request.urlopen(request, timeout=timeout, context=context) as response:
        body = response.read().decode("utf-8", "replace")
        return response.status, (json.loads(body) if body.strip() else {})


def read_braid(base_url, token):
    _, view = _request(base_url + BRAID_PATH, token=token)
    return view


def deliver_envelope(base_url, envelope, token):
    return _request(base_url + ENVELOPES_PATH, payload=envelope, token=token)


def read_missions(base_url, token):
    _, body = _request(base_url + MISSIONS_PATH, token=token)
    return body.get("missions", [])


# --------------------------------------------------------------------------
# The live run
# --------------------------------------------------------------------------

def run(args):
    base_url = args.agent_url.rstrip("/")
    token = args.token or os.environ.get("QUALIA_MISSION_BROKER_TOKEN", "") or None
    record = {"agent_url": base_url, "budget_mib": args.budget_mib, "legs": {}}

    # Leg: the agent answers and reports a braid at all.
    try:
        before_view = read_braid(base_url, token)
    except (urllib.error.URLError, urllib.error.HTTPError, OSError, ValueError) as error:
        return finish(args, record, {
            "agent": ("CANNOT-ASSERT", "%s%s did not answer: %s" % (base_url, BRAID_PATH, error)),
        })

    before = int(before_view.get("open_missions", 0))
    session = before_view.get("session_id")
    generation = before_view.get("generation")
    print("mission-check: agent %s answered; braid before: open_missions=%d session=%s generation=%s" % (
        base_url, before, session, generation))

    if not token:
        return finish(args, record, {
            "mission-drive": (
                "CANNOT-ASSERT",
                "no mission-broker token (--token or QUALIA_MISSION_BROKER_TOKEN); "
                "the agent never trusts loopback for mission intake",
            ),
        })

    mission_id = args.mission_id or "t29-board-%d" % int(time.time())
    envelope = build_envelope(mission_id, _now_ms(), args.deadline_s, idempotency_key=mission_id)
    cancel = build_cancel_envelope(mission_id, _now_ms(), args.deadline_s)
    violations = envelope_bounds_errors(envelope) + envelope_bounds_errors(cancel)
    if violations:
        return finish(args, record, {
            "mission-drive": ("CANNOT-ASSERT", "generated envelope is invalid: %s" % "; ".join(violations)),
        })

    try:
        status, ack = deliver_envelope(base_url, envelope, token)
    except urllib.error.HTTPError as error:
        body = error.read().decode("utf-8", "replace")
        return finish(args, record, {
            "mission-drive": ("FAIL", "envelope refused with HTTP %d: %s" % (error.code, body.strip())),
        })
    except (urllib.error.URLError, OSError, ValueError) as error:
        return finish(args, record, {
            "mission-drive": ("FAIL", "envelope delivery failed: %s" % error),
        })
    if status not in (200, 202):
        return finish(args, record, {"mission-drive": ("FAIL", "envelope answered HTTP %d" % status)})
    print("mission-check: mission %s accepted (HTTP %d, idempotent_replay=%s)" % (
        mission_id, status, ack.get("idempotent_replay")))

    samples, memories, read_failures, memory_note = _observe(
        base_url, token, before, args, mission_id, cancel
    )

    # Assertion 1: the braid transition, then the mission record behind it.
    verdict, detail = evaluate_transition(samples, before, args.open_timeout_s, args.close_timeout_s)
    if verdict == "FAIL" and read_failures:
        detail = "%s; braid reads failed: %s" % (detail, read_failures[0])
    terminal = None
    if verdict == "OK":
        terminal = _terminal_record(base_url, token, mission_id)
        if terminal is None:
            verdict, detail = "FAIL", "mission %s has no record on %s" % (mission_id, MISSIONS_PATH)
        elif terminal.get("status") not in TERMINAL_STATUSES:
            verdict, detail = "FAIL", "mission %s is not terminal: status=%s stage=%s" % (
                mission_id, terminal.get("status"), terminal.get("stage"))
        else:
            print("mission-check: mission record terminal: status=%s stage=%s code=%s detail=%s" % (
                terminal.get("status"), terminal.get("stage"), terminal.get("last_code"),
                terminal.get("last_detail")))

    # Assertion 2: the memory bound over the same window.
    peak = max((mib for _, mib, _ in memories), default=None)
    sources = sorted({source for _, _, source in memories})
    memory_verdict, memory_detail = evaluate_memory(peak, args.budget_mib)
    if memories:
        memory_detail = "%s over %d sample(s) from %s" % (memory_detail, len(memories), ", ".join(sources))
    elif memory_note:
        memory_detail = "%s (%s)" % (memory_detail, memory_note)

    record["legs"]["braid-terminal"] = {
        "before": before,
        "mission_id": mission_id,
        "samples": [[round(elapsed, 2), count] for elapsed, count in samples],
        "record": terminal,
        "verdict": verdict,
        "detail": detail,
    }
    record["legs"]["gpu-memory"] = {
        "peak_mib": peak,
        "samples": [[round(elapsed, 2), mib, source] for elapsed, mib, source in memories],
        "sources": sources,
        "note": memory_note,
        "verdict": memory_verdict,
        "detail": memory_detail,
    }

    if verdict == "OK":
        print("mission-check: braid-terminal OK (%s; session=%s generation=%s)" % (detail, session, generation))
    else:
        print("mission-check: braid-terminal %s (%s)" % (verdict, detail), file=sys.stderr)
    if memory_verdict == "OK":
        print("mission-check: gpu-memory OK (%s)" % memory_detail)
    else:
        print("mission-check: gpu-memory %s (%s)" % (memory_verdict, memory_detail), file=sys.stderr)

    return _outcome(args, record, {"braid-terminal": verdict, "gpu-memory": memory_verdict})


def _observe(base_url, token, before, args, mission_id, cancel):
    """Poll the braid through the mission's life, sampling memory alongside it.

    The mission is ended the way Step 28 ends it: once `open_missions` has risen,
    the second envelope cancels it, and the broker's termination is what returns
    the count. The start envelope's deadline is the backstop if that delivery is
    lost, so the loop is bounded either way.
    """
    start = time.monotonic()
    samples = []
    memories = []
    read_failures = []
    memory_note = ""
    next_memory_at = 0.0
    deadline = args.open_timeout_s + args.close_timeout_s
    cancelled = False
    hold_until = None

    while True:
        elapsed = time.monotonic() - start
        try:
            count = int(read_braid(base_url, token).get("open_missions", 0))
            samples.append((elapsed, count))
        except (urllib.error.URLError, urllib.error.HTTPError, OSError, ValueError) as error:
            count = None
            if len(read_failures) < 3:
                read_failures.append(str(error))
        if elapsed >= next_memory_at:
            mib, source, note = sample_memory(args.memory_source)
            if mib is None:
                if not memory_note:
                    memory_note = note
            else:
                memories.append((elapsed, mib, source))
            next_memory_at = elapsed + MEMORY_SAMPLE_INTERVAL_S
        if count is not None:
            opened = any(value > before for _, value in samples)
            if opened and count <= before:
                break
            if not opened and elapsed > args.open_timeout_s:
                break
            if opened and not cancelled:
                # Hold the mission open long enough for the memory sampler to see
                # the stack working, then end it the way Step 28 does.
                if hold_until is None:
                    hold_until = elapsed + args.hold_s
                if elapsed >= hold_until:
                    cancelled = True
                    try:
                        status, _ = deliver_envelope(base_url, cancel, token)
                        cancel_note = "cancel answered HTTP %d" % status
                    except (urllib.error.URLError, urllib.error.HTTPError, OSError, ValueError) as error:
                        cancel_note = "cancel delivery failed: %s (the start deadline closes it)" % error
                    print("mission-check: mission %s %s" % (mission_id, cancel_note))
        if elapsed > deadline:
            break
        time.sleep(POLL_INTERVAL_S)
    return samples, memories, read_failures, memory_note


def _terminal_record(base_url, token, mission_id):
    try:
        missions = read_missions(base_url, token)
    except (urllib.error.URLError, urllib.error.HTTPError, OSError, ValueError) as error:
        print("mission-check: could not read %s: %s" % (MISSIONS_PATH, error), file=sys.stderr)
        return None
    for mission in missions:
        if mission.get("mission_id") == mission_id:
            return mission
    return None


def _now_ms():
    return int(time.time() * 1000)


def finish(args, record, verdicts):
    """A leg that ended before the assertions could run: record it and report."""
    for leg, (verdict, detail) in verdicts.items():
        record["legs"][leg] = {"verdict": verdict, "detail": detail}
        print("mission-check: %s %s (%s)" % (leg, verdict, detail), file=sys.stderr)
    return _outcome(args, record, {leg: verdict for leg, (verdict, _) in verdicts.items()})


def _outcome(args, record, verdicts):
    record["verdict"] = verdicts
    if args.json_out:
        with open(args.json_out, "w", encoding="utf-8") as handle:
            json.dump(record, handle, indent=2, sort_keys=True)
            handle.write("\n")
    if "FAIL" in verdicts.values():
        failed = [name for name in verdicts if verdicts[name] == "FAIL"]
        print("mission: FAIL (%s)" % ", ".join(failed))
        return 1
    if "CANNOT-ASSERT" in verdicts.values():
        unjudged = [name for name in verdicts if verdicts[name] == "CANNOT-ASSERT"]
        print("mission: CANNOT-ASSERT (%s)" % ", ".join(unjudged))
        return 2
    print("mission: OK")
    return 0


# --------------------------------------------------------------------------
# The self-test: every pure function, on the host, without a board
# --------------------------------------------------------------------------

def self_test():
    cases = []
    failures = []

    def case(label, condition, detail=""):
        cases.append(label)
        if not condition:
            failures.append("%s%s" % (label, (": " + detail) if detail else ""))

    # The 1 -> 0 transition, and the ways it does not happen.
    verdict, detail = evaluate_transition([(0.0, 0), (1.0, 1), (2.0, 1), (21.0, 0)], 0, 10, 30)
    case("transition 0 -> 1 -> 0 is OK", verdict == "OK", detail)
    case("the transition detail names the sequence", "0 -> 1 -> 0" in detail, detail)
    verdict, _ = evaluate_transition([(0.0, 0), (30.0, 0)], 0, 10, 30)
    case("a mission that never opens fails", verdict == "FAIL")
    verdict, _ = evaluate_transition([(0.0, 0), (1.0, 1), (60.0, 1)], 0, 10, 30)
    case("a mission that never closes fails", verdict == "FAIL")
    verdict, detail = evaluate_transition([(0.0, 2), (1.0, 3), (9.0, 2)], 2, 10, 30)
    case("a repeat run with two missions already open still transitions", verdict == "OK", detail)
    verdict, _ = evaluate_transition([(0.0, 0), (12.0, 1), (13.0, 0)], 0, 10, 30)
    case("an open past the open window fails", verdict == "FAIL")
    verdict, detail = evaluate_transition([(0.0, 0), (1.0, 1), (50.0, 0)], 0, 10, 30)
    case("a close past the close window fails", verdict == "FAIL", detail)
    verdict, _ = evaluate_transition([], 0, 10, 30)
    case("no braid reads cannot be asserted", verdict == "CANNOT-ASSERT")

    # The memory bound, including its boundary and the unreadable case.
    case("4096 MiB is under the 8192 MiB budget", evaluate_memory(4096, 8192)[0] == "OK")
    case("8191 MiB is under the 8192 MiB budget", evaluate_memory(8191, 8192)[0] == "OK")
    case("8192 MiB is not under the 8192 MiB budget", evaluate_memory(8192, 8192)[0] == "FAIL")
    case("an unread memory value cannot be asserted", evaluate_memory(None, 8192)[0] == "CANNOT-ASSERT")

    # Both parsers, against the readings the two tools actually print.
    case("nvidia-smi's 412 parses", parse_nvidia_smi_used_mib("412") == 412)
    case("nvidia-smi's [N/A] does not parse", parse_nvidia_smi_used_mib("[N/A]") is None)
    case("an empty nvidia-smi reading does not parse", parse_nvidia_smi_used_mib("") is None)
    case(
        "tegrastats' RAM line parses",
        parse_tegrastats_used_mib("RAM 1573/3601MB (lfb 1x2MB) SWAP 109/14088MB (cached 0MB)")
        == 1573,
    )
    case("a tegrastats line with no RAM does not parse", parse_tegrastats_used_mib("GR3D_FREQ 0%") is None)

    # The envelope must satisfy the bounds the broker validates.
    envelope = build_envelope("t29-board-selftest", 1_700_000_000_000, DEFAULT_DEADLINE_S)
    case("the generated envelope satisfies the broker's bounds",
         not envelope_bounds_errors(envelope), "; ".join(envelope_bounds_errors(envelope)))
    case("the generated envelope opens a frontier mission", envelope["objective"]["kind"] == "explore_frontier")
    case("the generated envelope is a start with evidence",
         envelope["command"] == "start" and bool(envelope["evidence_refs"]))
    case("an over-long deadline is clamped", envelope_bounds_errors(build_envelope("m", 0, 9_999)) == [])
    unclamped = build_envelope("m", 0, 1)
    unclamped["deadline_ms"] = unclamped["issued_at_ms"] + MAX_DEADLINE_MS + 1
    case("the bounds check catches an over-long deadline", bool(envelope_bounds_errors(unclamped)))
    no_evidence = build_envelope("m", 0, 1)
    no_evidence["evidence_refs"] = []
    case("the bounds check catches a start with no evidence", bool(envelope_bounds_errors(no_evidence)))
    wide_area = build_envelope("m", 0, 1)
    wide_area["constraints"]["operating_area"]["max_x_m"] = 100.0
    case("the bounds check catches an over-wide area", bool(envelope_bounds_errors(wide_area)))

    cancel = build_cancel_envelope("t29-board-selftest", 1_700_000_000_000, DEFAULT_DEADLINE_S)
    case("the cancel envelope satisfies the broker's bounds",
         not envelope_bounds_errors(cancel), "; ".join(envelope_bounds_errors(cancel)))
    case("the cancel envelope is sequence 2 of the same mission",
         cancel["sequence"] == 2 and cancel["command"] == "cancel"
         and cancel["mission_id"] == envelope["mission_id"]
         and cancel["producer_epoch"] == envelope["producer_epoch"])
    case("the cancel envelope carries its own idempotency key",
         cancel["idempotency_key"] != envelope["idempotency_key"])

    if failures:
        for failure in failures:
            sys.stderr.write("mission-check: self-test FAIL - %s\n" % failure)
        return 1
    print("mission-check: self-test OK (%d cases)" % len(cases))
    return 0


# --------------------------------------------------------------------------

def main(argv):
    parser = argparse.ArgumentParser(
        description="Step 29's two assertions (ticket #45): the braid's terminal state and the memory bound."
    )
    parser.add_argument("--agent-url", default="https://127.0.0.1:8081",
                        help="agent base URL (default: https://127.0.0.1:8081)")
    parser.add_argument("--token", default=None,
                        help="mission-broker bearer token (default: $QUALIA_MISSION_BROKER_TOKEN)")
    parser.add_argument("--mission-id", default=None, help="mission id (default: t29-board-<epoch>)")
    parser.add_argument("--deadline-s", type=int, default=DEFAULT_DEADLINE_S,
                        help="mission deadline in seconds (default: %d)" % DEFAULT_DEADLINE_S)
    parser.add_argument("--open-timeout-s", type=float, default=DEFAULT_OPEN_TIMEOUT_S,
                        help="seconds to wait for the mission to open (default: %.0f)" % DEFAULT_OPEN_TIMEOUT_S)
    parser.add_argument("--close-timeout-s", type=float, default=DEFAULT_CLOSE_TIMEOUT_S,
                        help="seconds to wait for the mission to close (default: %.0f)" % DEFAULT_CLOSE_TIMEOUT_S)
    parser.add_argument("--hold-s", type=float, default=DEFAULT_HOLD_S,
                        help="seconds to hold the mission open before cancelling it (default: %.0f)" % DEFAULT_HOLD_S)
    parser.add_argument("--budget-mib", type=int, default=DEFAULT_BUDGET_MIB,
                        help="the memory bound in MiB (default: %d, step 29's 8 GB)" % DEFAULT_BUDGET_MIB)
    parser.add_argument("--memory-source", choices=("auto", "nvidia-smi", "tegrastats"), default="auto",
                        help="where memory is read (default: auto — nvidia-smi, then tegrastats)")
    parser.add_argument("--json", dest="json_out", default=None, help="write the observation record here")
    parser.add_argument("--self-test", action="store_true",
                        help="run the pure-function fixtures; no board needed")
    args = parser.parse_args(argv[1:])

    if args.self_test:
        return self_test()
    if args.deadline_s < 1:
        parser.print_usage(sys.stderr)
        sys.stderr.write("mission-check: usage error - --deadline-s must be at least 1\n")
        return 2
    return run(args)


if __name__ == "__main__":
    sys.exit(main(sys.argv))
