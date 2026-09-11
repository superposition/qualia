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
import math
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

JSON_SAFE_INTEGER_MAX = (1 << 53) - 1
IDENTIFIER_EXTRA = "-_.:"
ENVELOPE_KEYS = (
    "schema_version", "broker_id", "producer_epoch", "sequence", "mission_id",
    "idempotency_key", "command", "issued_at_ms", "deadline_ms", "objective",
    "constraints", "evidence_refs", "fly_governed",
)
OBJECTIVE_KEYS = ("kind", "summary", "target_x_m", "target_y_m", "tolerance_m")
CONSTRAINT_KEYS = (
    "operating_area", "speed_ceiling_mps", "max_distance_m", "max_runtime_ms",
    "max_replans", "evidence_max_age_ms",
)
AREA_KEYS = ("frame_id", "min_x_m", "min_y_m", "max_x_m", "max_y_m")
COMMANDS = ("start", "pause", "resume", "cancel")
POINT_KINDS = ("observe_point", "navigate_to")

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
    """The broker's `validate()` bounds, restated as a list of violations.

    A field-for-field mirror of `MissionEnvelopeV1::validate()` in
    `crates/sync-types/src/mission.rs`, including the `deny_unknown_fields` key
    sets and `validate_identifier`'s alphabet: the broker refuses an envelope
    that trips any of these with a `400`, so the self-test runs this over the
    generated pair and a drift either side fails on the host, before a board run.
    """
    errors = []

    def identifier(field, value):
        if (
            not value
            or len(value) > 160
            or not all(
                character.isascii() and (character.isalnum() or character in IDENTIFIER_EXTRA)
                for character in value
            )
        ):
            errors.append("%s must contain 1..=160 URL-safe characters" % field)

    errors.extend(
        "unexpected envelope field %r" % key for key in sorted(set(envelope) - set(ENVELOPE_KEYS))
    )
    if envelope.get("schema_version") != MISSION_ENVELOPE_SCHEMA:
        errors.append("unsupported envelope schema")
    for field in ("broker_id", "mission_id", "idempotency_key"):
        identifier(field, envelope.get(field, ""))
    epoch = envelope.get("producer_epoch", 0)
    sequence = envelope.get("sequence", 0)
    if not 0 < epoch <= JSON_SAFE_INTEGER_MAX or not 0 < sequence <= JSON_SAFE_INTEGER_MAX:
        errors.append("producer epoch and sequence must be non-zero JSON-safe integers")
    issued = envelope.get("issued_at_ms", 0)
    deadline = envelope.get("deadline_ms", 0)
    if (
        issued == 0
        or issued > JSON_SAFE_INTEGER_MAX
        or deadline > JSON_SAFE_INTEGER_MAX
        or deadline <= issued
        or deadline - issued > MAX_DEADLINE_MS
    ):
        errors.append("deadline must be within %d ms of issue" % MAX_DEADLINE_MS)
    if envelope.get("command") not in COMMANDS:
        errors.append("command must be one of %s" % ", ".join(COMMANDS))

    objective = envelope.get("objective", {})
    errors.extend(
        "unexpected objective field %r" % key for key in sorted(set(objective) - set(OBJECTIVE_KEYS))
    )
    summary = objective.get("summary", "")
    if not summary.strip() or len(summary) > 512:
        errors.append("objective summary must contain 1..=512 characters")
    x, y = objective.get("target_x_m"), objective.get("target_y_m")
    if objective.get("kind") in POINT_KINDS and (x is None or y is None):
        errors.append("point objectives require target_x_m and target_y_m")
    if (x is None) != (y is None):
        errors.append("mission target coordinates must be supplied together")
    for value in (x, y):
        if value is not None and not math.isfinite(value):
            errors.append("mission target coordinates must be finite")
    tolerance = objective.get("tolerance_m")
    if tolerance is not None and (not math.isfinite(tolerance) or not 0.05 <= tolerance <= 1.0):
        errors.append("mission tolerance_m must be in 0.05..=1.0")

    constraints = envelope.get("constraints", {})
    errors.extend(
        "unexpected constraint field %r" % key
        for key in sorted(set(constraints) - set(CONSTRAINT_KEYS))
    )
    area = constraints.get("operating_area", {})
    errors.extend(
        "unexpected operating-area field %r" % key for key in sorted(set(area) - set(AREA_KEYS))
    )
    corners = [area.get(key) for key in ("min_x_m", "min_y_m", "max_x_m", "max_y_m")]
    if (
        area.get("frame_id") != "odom"
        or any(corner is None or not math.isfinite(corner) for corner in corners)
        or area.get("min_x_m", 0) >= area.get("max_x_m", 0)
        or area.get("min_y_m", 0) >= area.get("max_y_m", 0)
        or area.get("max_x_m", 0) - area.get("min_x_m", 0) > MAX_AREA_SPAN_M
        or area.get("max_y_m", 0) - area.get("min_y_m", 0) > MAX_AREA_SPAN_M
    ):
        errors.append("operating area must be a finite odom rectangle no larger than %.0f m" % MAX_AREA_SPAN_M)
    if x is not None and y is not None and not (
        area.get("min_x_m", 0) <= x <= area.get("max_x_m", 0)
        and area.get("min_y_m", 0) <= y <= area.get("max_y_m", 0)
    ):
        errors.append("mission target is outside the bounded operating area")
    speed = constraints.get("speed_ceiling_mps")
    distance = constraints.get("max_distance_m")
    runtime = constraints.get("max_runtime_ms")
    replans = constraints.get("max_replans")
    age = constraints.get("evidence_max_age_ms")
    if (
        speed is None or not math.isfinite(speed) or not MIN_SPEED_MPS <= speed <= MAX_SPEED_MPS
        or distance is None or not math.isfinite(distance) or not MIN_DISTANCE_M <= distance <= MAX_DISTANCE_M
        or runtime is None or not MIN_RUNTIME_MS <= runtime <= MAX_RUNTIME_MS
        or replans is None or replans > MAX_REPLANS
        or age is None or not MIN_EVIDENCE_MAX_AGE_MS <= age <= MAX_EVIDENCE_MAX_AGE_MS
    ):
        errors.append("mission constraints exceed the bounded low-speed limits")

    refs = envelope.get("evidence_refs")
    if refs is None or len(refs) > 64 or any(not ref.strip() or len(ref) > 256 for ref in refs):
        errors.append("mission evidence_refs are invalid")
    elif envelope.get("command") == "start" and not refs:
        errors.append("a mission start requires at least one evidence reference")
    return errors


def record_mission_id(record):
    """The mission id of one `/mission-control/missions` record.

    `MissionRecordV1` (`runners/agent/src/mission_control.rs`) nests the accepted
    envelope, so the id lives at `record["envelope"]["mission_id"]`; a flat
    `mission_id` is accepted too, because an operator tool speaking the same wire
    contract may report the record flattened.
    """
    envelope = record.get("envelope")
    if isinstance(envelope, dict) and envelope.get("mission_id"):
        return envelope["mission_id"]
    return record.get("mission_id")


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
        if record_mission_id(mission) == mission_id:
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
        parent = os.path.dirname(os.path.abspath(args.json_out))
        if parent and not os.path.isdir(parent):
            os.makedirs(parent, exist_ok=True)
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

    # The envelope must satisfy the bounds the broker validates. Each mutation
    # asserts the violation it should raise, so a check that stops working fails
    # here rather than passing on some other complaint.
    issued = 1_700_000_000_000
    envelope = build_envelope("t29-board-selftest", issued, DEFAULT_DEADLINE_S)
    case("the generated envelope satisfies the broker's bounds",
         not envelope_bounds_errors(envelope), "; ".join(envelope_bounds_errors(envelope)))
    case("the generated envelope opens a frontier mission", envelope["objective"]["kind"] == "explore_frontier")
    case("the generated envelope is a start with evidence",
         envelope["command"] == "start" and bool(envelope["evidence_refs"]))
    case("an over-long deadline is clamped",
         not envelope_bounds_errors(build_envelope("m", issued, 9_999)))
    unclamped = build_envelope("m", issued, 1)
    unclamped["deadline_ms"] = unclamped["issued_at_ms"] + MAX_DEADLINE_MS + 1
    case("the bounds check catches an over-long deadline",
         any("deadline" in error for error in envelope_bounds_errors(unclamped)))
    no_evidence = build_envelope("m", issued, 1)
    no_evidence["evidence_refs"] = []
    case("the bounds check catches a start with no evidence",
         any("evidence reference" in error for error in envelope_bounds_errors(no_evidence)))
    wide_area = build_envelope("m", issued, 1)
    wide_area["constraints"]["operating_area"]["max_x_m"] = 100.0
    case("the bounds check catches an over-wide area",
         any("operating area" in error for error in envelope_bounds_errors(wide_area)))

    cancel = build_cancel_envelope("t29-board-selftest", issued, DEFAULT_DEADLINE_S)
    case("the cancel envelope satisfies the broker's bounds",
         not envelope_bounds_errors(cancel), "; ".join(envelope_bounds_errors(cancel)))
    case("the cancel envelope is sequence 2 of the same mission",
         cancel["sequence"] == 2 and cancel["command"] == "cancel"
         and cancel["mission_id"] == envelope["mission_id"]
         and cancel["producer_epoch"] == envelope["producer_epoch"])
    case("the cancel envelope carries its own idempotency key",
         cancel["idempotency_key"] != envelope["idempotency_key"])

    # Bounds the broker enforces beyond the ranges: the key sets it decodes with
    # `deny_unknown_fields`, the identifier alphabet and the point-objective rules.
    extra = build_envelope("m", issued, 1)
    extra["surprise"] = 1
    case("the bounds check catches an unknown envelope field",
         any("unexpected envelope field" in error for error in envelope_bounds_errors(extra)))
    bad_id = build_envelope("m", issued, 1)
    bad_id["mission_id"] = "not url safe/at all"
    case("the bounds check catches a non-URL-safe identifier",
         any("URL-safe" in error for error in envelope_bounds_errors(bad_id)))
    point = build_envelope("m", issued, 1)
    point["objective"]["kind"] = "navigate_to"
    case("the bounds check catches a point objective with no target",
         any("point objective" in error for error in envelope_bounds_errors(point)))
    outside = build_envelope("m", issued, 1)
    outside["objective"]["target_x_m"], outside["objective"]["target_y_m"] = 40.0, 0.0
    case("the bounds check catches a target outside the area",
         any("outside the bounded operating area" in error for error in envelope_bounds_errors(outside)))
    inside = build_envelope("m", issued, 1)
    inside["objective"]["kind"] = "navigate_to"
    inside["objective"]["target_x_m"], inside["objective"]["target_y_m"] = 1.0, -2.0
    case("a target inside the area is legal", not envelope_bounds_errors(inside),
         "; ".join(envelope_bounds_errors(inside)))
    no_command = build_envelope("m", issued, 1)
    no_command["command"] = "teleport"
    case("the bounds check catches an unknown command",
         any("command must be one of" in error for error in envelope_bounds_errors(no_command)))

    # The record shape: MissionRecordV1 nests the envelope, which is the shape the
    # live agent answers with (`/mission-control/missions`).
    nested = {"envelope": {"mission_id": "m-nested"}, "status": "cancelled", "stage": "terminal",
              "last_code": "cancelled", "last_detail": "broker cancelled the mission"}
    flat = {"mission_id": "m-flat", "status": "failed", "stage": "terminal",
            "last_code": "deadline_exceeded"}
    case("a nested record reports its envelope's mission id", record_mission_id(nested) == "m-nested")
    case("a flat record reports its own mission id", record_mission_id(flat) == "m-flat")
    case("a record with no id reports none", record_mission_id({"status": "cancelled"}) is None)
    case("the terminal record shapes are the statuses the checker accepts",
         nested["status"] in TERMINAL_STATUSES and flat["status"] in TERMINAL_STATUSES)

    # The terminal-record lookup must find a nested record, which is what the live
    # agent answers: the regression that a flat-only match hides.
    found = next(
        (mission for mission in [{"envelope": {"mission_id": "keep"}}, nested]
         if record_mission_id(mission) == "m-nested"),
        None,
    )
    case("the lookup finds a nested record among others", found is nested)

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
