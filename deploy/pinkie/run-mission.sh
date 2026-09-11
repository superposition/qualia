#!/usr/bin/env bash
# T29 / EPIC-10 step 29 — the board mission run on Pinkie (the Jetson Orin NX).
#
# Step 29: "Rebuild the fatbin per Step 18 (`CUDAARCHS=87-real`), copy the prior
# directory, run the same zero-motion stack with `QUALIA_CUDA_SM=87`, and assert
# `nvidia-smi` reports under 8 GB used and that `GET /braid` reaches the same
# terminal state."
#
# This is the board half. The dev host cannot cross-build today (D-018: the
# Docker engine is unreachable, `cross` is not installed, no aarch64 gcc), so the
# artifact is built natively here, from the `git archive` the host shipped with
# `deploy/pinkie/ship-mission.sh` — the lane D-010/D-018 name.
#
# The stack manifest is authoritative for the `env` block it declares; this
# script's environment only fills in what the manifest leaves unset. The features
# each runner is built with are read from that runner's own manifest: the layer
# runners default to the macOS `metal` backend, so a board build must ask for
# `cuda` explicitly, and each belief layer's prior coupling is opt-in.
#
# The two assertions are `scripts/mission_check.py`'s (Step 29's two): the
# `GET /braid` open -> close transition with a terminal mission record, and the
# peak memory under 8 GB. What cannot be judged (no readable memory counter, no
# broker token) is reported as such and exits 2 — an unjudged run is not a pass.
#
# Usage:
#   bash deploy/pinkie/run-mission.sh --plan    # print the exact commands, run nothing
#   bash deploy/pinkie/run-mission.sh --check   # preflight only
#   bash deploy/pinkie/run-mission.sh           # build, run, assert, stop
#
# Options:
#   --repo DIR        the shipped tree (default: two levels above this script)
#   --manifest PATH   the ticket's zero-motion manifest (default: config/stack-manifest.zero-motion.json)
#   --prior DIR       the prior directory to load (default: assets/brain/prior)
#   --web-port N      the agent's port (default: 8081; the board's 8000 is Leash, 8080 llama.cpp)
#   --sm N            QUALIA_CUDA_SM (default: 87)
#   --leash-url URL   QUALIA_LEASH_BASE_URL (default: http://127.0.0.1:8000)
#   --deadline-s N    the mission's bounded deadline (default: 20)
#   --budget-mib N    the memory bound in MiB (default: 8192, Step 29's 8 GB)
#   --run-dir DIR     where logs and evidence land (default: ~/qualia-board/runs/<UTC stamp>)
#   --skip-build      reuse an existing target/release
#   --keep-stack      leave the stack running when the assertions finish
#
# Exit codes: 0 both assertions hold; 1 an assertion failed; 2 preflight refused
# (or `--check`); 3 the build failed; 4 the stack could not start or stop.

set -euo pipefail

PYTHON="${PYTHON:-python3}"
SM=87
WEB_PORT=8081
LEASH_URL="http://127.0.0.1:8000"
DEADLINE_S=20
BUDGET_MIB=8192
MODE=run
SKIP_BUILD=0
KEEP_STACK=0
REPO=""
MANIFEST=""
PRIOR=""
RUN_DIR=""

while [ $# -gt 0 ]; do
  case "$1" in
    --repo) REPO="$2"; shift 2 ;;
    --manifest) MANIFEST="$2"; shift 2 ;;
    --prior) PRIOR="$2"; shift 2 ;;
    --web-port) WEB_PORT="$2"; shift 2 ;;
    --sm) SM="$2"; shift 2 ;;
    --leash-url) LEASH_URL="$2"; shift 2 ;;
    --deadline-s) DEADLINE_S="$2"; shift 2 ;;
    --budget-mib) BUDGET_MIB="$2"; shift 2 ;;
    --run-dir) RUN_DIR="$2"; shift 2 ;;
    --skip-build) SKIP_BUILD=1; shift ;;
    --keep-stack) KEEP_STACK=1; shift ;;
    --plan) MODE=plan; shift ;;
    --check) MODE=check; shift ;;
    -h|--help) sed -n '2,42p' "$0"; exit 0 ;;
    *) echo "run-mission: unknown option: $1" >&2; exit 2 ;;
  esac
done

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
: "${REPO:=$(cd -- "$SCRIPT_DIR/../.." && pwd)}"
: "${MANIFEST:=$REPO/config/stack-manifest.zero-motion.json}"
: "${PRIOR:=$REPO/assets/brain/prior}"
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
: "${RUN_DIR:=$HOME/qualia-board/runs/$STAMP}"
MISSION_ID="t29-board-$STAMP"
TOKEN="$(head -c 16 /dev/urandom | od -An -tx1 | tr -d ' \n')"

AGENT_URL="https://127.0.0.1:$WEB_PORT"
BIN_DIR="$REPO/target/release"
STACK_PID=""
HARD_FAILURES=0
SOFT_WARNINGS=0
ASSERT_STATUS=0

say()   { printf '%s\n' "$*"; }
head2() { printf '\n== %s ==\n' "$*"; }

record() {
  # record <label> <exit-status> [detail]
  if [ "$2" -eq 0 ]; then
    say "ok   $1${3:+ ($3)}"
  else
    say "FAIL $1${3:+ ($3)} (exit=$2)"
    HARD_FAILURES=$((HARD_FAILURES + 1))
  fi
}

warn() { say "warn $1${2:+ ($2)}"; SOFT_WARNINGS=$((SOFT_WARNINGS + 1)); }

probe() {
  # probe <label> <soft|hard> <command...>
  local label="$1" weight="$2" status=0 detail=""
  shift 2
  detail="$("$@" 2>&1)" || status=$?
  if [ "$status" -eq 0 ]; then
    say "ok   $label${detail:+ (${detail%%$'\n'*})}"
  elif [ "$weight" = "soft" ]; then
    warn "$label" "${detail:-no output}"
  else
    record "$label" "$status" "${detail:-no output}"
  fi
}

# --------------------------------------------------------------------------

preflight() {
  head2 "preflight"
  say "repo:     $REPO"
  say "manifest: $MANIFEST"
  say "prior:    $PRIOR"
  say "agent:    $AGENT_URL"
  say "run dir:  $RUN_DIR"
  say "python:   $PYTHON"
  say ""

  local arch
  arch="$(uname -m)"
  if [ "$arch" = "aarch64" ]; then
    say "ok   the board is aarch64 ($arch)"
  else
    record "uname -m is aarch64" 1 "$arch"
  fi

  export PATH="$HOME/.cargo/bin:$PATH"
  probe "python3 is present" hard "$PYTHON" --version
  probe "cargo is on PATH" hard cargo --version
  probe "rustc is on PATH" hard rustc --version
  probe "make is present" hard make --version
  probe "nvcc is present (Step 18's rebuild)" hard /usr/local/cuda/bin/nvcc --version
  probe "nvidia-smi is present" soft nvidia-smi --query-gpu=name --format=csv,noheader
  probe "tegrastats is present (the Jetson memory source)" soft tegrastats --help

  if [ -f "$MANIFEST" ]; then
    local names=""
    names="$("$PYTHON" - "$MANIFEST" <<'PY'
import json, sys
manifest = json.load(open(sys.argv[1]))
print(",".join(runner["name"] for runner in manifest["runners"]))
PY
    )" || names=""
    if [ -n "$names" ]; then
      say "ok   the manifest parses (runners: $names)"
      case ",$names," in
        *,qualia-agent,*) say "ok   the manifest names qualia-agent (the /braid surface)" ;;
        *) record "the manifest names qualia-agent" 1 \
             "T28 #44 completes config/stack-manifest.zero-motion.json" ;;
      esac
    else
      record "the manifest parses" 1 "not the qualia.stack.v1 JSON"
    fi
  else
    record "the manifest exists" 1 "$MANIFEST"
  fi

  probe "the prior directory holds graph.bin" hard test -f "$PRIOR/graph.bin"
  probe "the prior directory holds manifest.json" hard test -f "$PRIOR/manifest.json"

  local port_free=0
  "$PYTHON" - "$WEB_PORT" <<'PY' >/dev/null 2>&1 || port_free=$?
import socket, sys
port = int(sys.argv[1])
with socket.socket() as probe_socket:
    try:
        probe_socket.bind(("127.0.0.1", port))
    except OSError as error:
        sys.stderr.write("port %d is busy: %s" % (port, error))
        sys.exit(1)
PY
  record "port $WEB_PORT is free for the agent" "$port_free"

  local mem=""
  mem="$(free -m 2>/dev/null | awk '/^Mem:/ {print $7}')" || mem=""
  if [ -n "$mem" ] && [ "$mem" -lt 1200 ] 2>/dev/null; then
    warn "available memory is low" "${mem} MiB"
  else
    say "ok   available memory${mem:+ ($mem MiB)}"
  fi

  say ""
  say "preflight: $HARD_FAILURES hard failure(s), $SOFT_WARNINGS warning(s)"
}

feature_groups() {
  "$PYTHON" - "$REPO" "$MANIFEST" <<'PY'
import json, os, re, sys

repo, manifest_path = sys.argv[1], sys.argv[2]
wanted = []
if os.path.isfile(manifest_path):
    manifest = json.load(open(manifest_path))
    wanted = [runner["name"] for runner in manifest["runners"]]
for required in ("qualia-init", "qualia-cli"):
    if required not in wanted:
        wanted.append(required)

# The workspace's own member list is the index: no walk, no guessing.
root = open(os.path.join(repo, "Cargo.toml"), encoding="utf-8").read()
members = re.search(r"members\s*=\s*\[([^\]]*)\]", root, re.S)
member_paths = re.findall(r'"([^"]+)"', members.group(1)) if members else []

by_package = {}
for member in member_paths:
    path = os.path.join(repo, member, "Cargo.toml")
    if not os.path.isfile(path):
        continue
    text = open(path, encoding="utf-8").read()
    package = re.search(r'^name\s*=\s*"([^"]+)"', text, re.M)
    if package:
        by_package[package.group(1)] = text

groups = {}
unknown = []
for name in wanted:
    text = by_package.get(name)
    if text is None:
        unknown.append(name)
        continue
    features = ""
    default = re.search(r"^default\s*=\s*\[([^\]]*)\]", text, re.M)
    default_features = default.group(1) if default else ""
    if re.search(r"^cuda\s*=", text, re.M) and "cuda" not in default_features:
        features = "cuda"
        if re.search(r"^fly-prior\s*=", text, re.M):
            features += ",fly-prior"
    groups.setdefault(features, []).append(name)

if unknown:
    sys.stderr.write(
        "feature_groups: no workspace member for %s\n" % ", ".join(unknown)
    )
    sys.exit(3)

for features, names in sorted(groups.items()):
    flags = " ".join("-p %s" % name for name in sorted(names))
    feature_flag = ("--no-default-features --features %s" % features) if features else ""
    print("cargo build --release --offline -j 2 %s %s" % (flags, feature_flag))
PY
}

plan() {
  head2 "plan"
  say "build:"
  say "  1. make cuda-fatbin-87   (Step 18's rebuild; CUDAARCHS=87-real, NVCC=/usr/local/cuda/bin/nvcc)"
  while IFS= read -r line; do
    [ -n "$line" ] && say "  2. $line"
  done < <(feature_groups)
  say ""
  say "run:"
  say "  # the manifest's env block wins over the shell except for env_passthrough keys,"
  say "  # which is where these land (cuda-service: SM; explore/agent: fly mode + prior;"
  say "  # agent: port/token/journal/store/tls; arena-recorder: mcap root + session)"
  say "  QUALIA_STACK_MANIFEST=$MANIFEST QUALIA_CUDA_SM=$SM QUALIA_FLY_MODE=prior \\"
  say "  QUALIA_FLY_PRIOR_PATH=$PRIOR QUALIA_WEB_PORT=$WEB_PORT QUALIA_LEASH_BASE_URL=$LEASH_URL \\"
  say "  QUALIA_COMPUTE_SOCKET=$RUN_DIR/qualia-compute.sock \\"
  say "  QUALIA_MISSION_BROKER_TOKEN=<generated 32 hex> QUALIA_LOG_DIR=$RUN_DIR/logs \\"
  say "  QUALIA_MISSION_CONTROL_JOURNAL=$RUN_DIR/mission-control.jsonl \\"
  say "  QUALIA_SESSION_STORE=$RUN_DIR/sessions.sqlite QUALIA_TLS_DIR=$RUN_DIR/tls \\"
  say "  QUALIA_MCAP_ROOT=$RUN_DIR/mcap QUALIA_ARENA_SESSION=$MISSION_ID \\"
  say "  $BIN_DIR/qualia run --manifest $MANIFEST"
  say ""
  say "assert (Step 29's two):"
  say "  $PYTHON scripts/mission_check.py --agent-url $AGENT_URL --token <generated> \\"
  say "    --mission-id $MISSION_ID --deadline-s $DEADLINE_S --budget-mib $BUDGET_MIB \\"
  say "    --json $RUN_DIR/mission.json"
  say ""
  say "stop:"
  say "  $BIN_DIR/qualia stop --manifest $MANIFEST   (SIGTERM lets the arena recorder seal)"
}

show_build_tail() {
  [ "$1" -eq 0 ] || { say "--- build log tail ---"; tail -15 "$RUN_DIR/build.log" 2>/dev/null || true; }
}

build_stack() {
  head2 "build"
  export PATH="$HOME/.cargo/bin:$PATH"
  export CARGO_BUILD_JOBS=2

  local groups=""
  groups="$(feature_groups)" || {
    record "the build plan (the manifest's runners are workspace members)" 1 "see the message above"
    return 3
  }
  if [ -z "$groups" ]; then
    record "the build plan is not empty" 1 "no package to build"
    return 3
  fi

  local status=0
  say "\$ make cuda-fatbin-87"
  make cuda-fatbin-87 >>"$RUN_DIR/build.log" 2>&1 || status=1
  record "make cuda-fatbin-87 (Step 18's rebuild)" "$status"
  show_build_tail "$status"

  local line
  while IFS= read -r line; do
    [ -n "$line" ] || continue
    say "\$ $line"
    status=0
    # The command is built by feature_groups from known package names and flags.
    # shellcheck disable=SC2086
    $line >>"$RUN_DIR/build.log" 2>&1 || status=1
    record "cargo build ${line#cargo build --release --offline -j 2 }" "$status"
    show_build_tail "$status"
  done <<<"$groups"

  say ""
  if [ "$HARD_FAILURES" -gt 0 ]; then
    say "build: failed ($HARD_FAILURES)"
    return 3
  fi
  say "build: ok"
  return 0
}

# --------------------------------------------------------------------------

probe_braid() {
  "$PYTHON" - "$AGENT_URL" <<'PY'
import json, ssl, sys, urllib.request
url = sys.argv[1].rstrip("/") + "/braid"
context = ssl._create_unverified_context() if url.startswith("https") else None
with urllib.request.urlopen(urllib.request.Request(url), timeout=5, context=context) as reply:
    print(json.loads(reply.read().decode())["open_missions"])
PY
}

start_stack() {
  head2 "run"
  mkdir -p "$RUN_DIR/logs" "$RUN_DIR/mcap" "$RUN_DIR/tls"
  export QUALIA_STACK_MANIFEST="$MANIFEST"
  # The manifest's `env` block wins over the shell, except for the keys each
  # runner lists in `env_passthrough` — and init applies those after the stack
  # env. These are passthrough keys, so the board's values win: SM 87 rather
  # than the host run's 89, an absolute prior path, and this run's own ports,
  # compute socket, journal, store and certificate.
  export QUALIA_CUDA_SM="$SM"
  export QUALIA_FLY_MODE=prior
  export QUALIA_FLY_PRIOR_PATH="$PRIOR"
  export QUALIA_WEB_PORT="$WEB_PORT"
  export QUALIA_COMPUTE_SOCKET="$RUN_DIR/qualia-compute.sock"
  export QUALIA_LEASH_BASE_URL="$LEASH_URL"
  export QUALIA_MISSION_BROKER_TOKEN="$TOKEN"
  export QUALIA_LOG_DIR="$RUN_DIR/logs"
  export QUALIA_MISSION_CONTROL_JOURNAL="$RUN_DIR/mission-control.jsonl"
  export QUALIA_SESSION_STORE="$RUN_DIR/sessions.sqlite"
  export QUALIA_TLS_DIR="$RUN_DIR/tls"
  export QUALIA_MCAP_ROOT="$RUN_DIR/mcap"
  export QUALIA_ARENA_SESSION="$MISSION_ID"

  say "\$ $BIN_DIR/qualia run --manifest $MANIFEST   (background)"
  "$BIN_DIR/qualia" run --manifest "$MANIFEST" >"$RUN_DIR/stack.out" 2>&1 &
  STACK_PID=$!
  say "     pid $STACK_PID"

  local waited=0
  while [ "$waited" -lt 60 ]; do
    if probe_braid >/dev/null 2>&1; then
      say "ok   $AGENT_URL/braid answered after ${waited}s"
      return 0
    fi
    if ! kill -0 "$STACK_PID" 2>/dev/null; then
      say "FAIL the supervisor exited during start-up"
      tail -20 "$RUN_DIR/stack.out" 2>/dev/null || true
      return 4
    fi
    sleep 1
    waited=$((waited + 1))
  done
  say "FAIL $AGENT_URL/braid did not answer within 60s"
  tail -20 "$RUN_DIR/stack.out" 2>/dev/null || true
  return 4
}

assert_mission() {
  head2 "assert (Step 29's two)"
  local status=0
  "$PYTHON" "$REPO/scripts/mission_check.py" \
    --agent-url "$AGENT_URL" --token "$TOKEN" --mission-id "$MISSION_ID" \
    --deadline-s "$DEADLINE_S" --budget-mib "$BUDGET_MIB" \
    --json "$RUN_DIR/mission.json" || status=$?
  ASSERT_STATUS=$status
  case "$status" in
    0) say "assert: ok" ;;
    1) say "assert: an assertion failed" ;;
    2) say "assert: a leg could not be judged" ;;
    *) say "assert: mission_check exited $status" ;;
  esac
}

stop_stack() {
  head2 "stop"
  local status=0
  "$BIN_DIR/qualia" stop --manifest "$MANIFEST" || status=$?
  say "qualia stop exit=$status"

  local waited=0
  while [ "$waited" -lt 15 ] && kill -0 "$STACK_PID" 2>/dev/null; do
    sleep 1
    waited=$((waited + 1))
  done
  if kill -0 "$STACK_PID" 2>/dev/null; then
    warn "the supervisor did not stop after ${waited}s; sending SIGTERM"
    kill -TERM "$STACK_PID" 2>/dev/null || true
    waited=0
    while [ "$waited" -lt 10 ] && kill -0 "$STACK_PID" 2>/dev/null; do
      sleep 1
      waited=$((waited + 1))
    done
  fi
  if kill -0 "$STACK_PID" 2>/dev/null; then
    record "the stack stopped" 1 "supervisor pid $STACK_PID still alive"
    # `pkill -x`, never `-f`: a `-f` pattern can match this script's own ssh line.
    pkill -x qualia-init 2>/dev/null || true
  else
    say "ok   the stack stopped (control socket, qualia stop exit=$status)"
  fi

  if [ -d "$RUN_DIR/mcap" ]; then
    say "mcap root $RUN_DIR/mcap:"
    find "$RUN_DIR/mcap" -type f -printf '  %s  %p\n' 2>/dev/null | sort || say "  (empty)"
  fi
  if [ -f "$RUN_DIR/mission-control.jsonl" ]; then
    say "mission journal: $(wc -l <"$RUN_DIR/mission-control.jsonl") line(s)"
  fi
}

summary() {
  head2 "summary"
  say "mission id:   $MISSION_ID"
  say "assert:       $ASSERT_STATUS (0 ok, 1 failed, 2 unjudged)"
  say "preflight:    $HARD_FAILURES hard failure(s), $SOFT_WARNINGS warning(s)"
  say "evidence:     $RUN_DIR/evidence.txt"
  say "record:       $RUN_DIR/mission.json"
  say "stack output: $RUN_DIR/stack.out"
  say "runner logs:  $RUN_DIR/logs/"
  say ""
  say "copy the evidence back with:"
  say "  scp -i ~/.ssh/qualia_jetson_ed25519 jetson@192.168.55.1:$RUN_DIR/evidence.txt /tmp/T29-evidence.txt"
  if [ "$ASSERT_STATUS" -eq 0 ]; then
    say "mission: PASS"
    return 0
  fi
  if [ "$ASSERT_STATUS" -eq 2 ]; then
    say "mission: CANNOT-ASSERT (a leg the board could not judge; assert exit 2)"
  else
    say "mission: FAIL (assert exit $ASSERT_STATUS)"
  fi
  return "$ASSERT_STATUS"
}

# --------------------------------------------------------------------------

main() {
  cd "$REPO"
  # `--plan` and `--check` inspect and print; they create nothing and write nothing.
  if [ "$MODE" = "run" ]; then
    mkdir -p "$RUN_DIR"
    : >"$RUN_DIR/evidence.txt"
    exec > >(tee -a "$RUN_DIR/evidence.txt") 2>&1
  fi
  say "== T29 board mission run (EPIC-10 step 29) =="
  say "date: $(date -u +%Y-%m-%dT%H:%M:%SZ)  host: $(uname -n) $(uname -m)  tree: $(pwd)"

  preflight
  if [ "$MODE" = "plan" ] || [ "$MODE" = "check" ]; then
    plan
    say ""
    say "check: $HARD_FAILURES hard failure(s), $SOFT_WARNINGS warning(s)"
    if [ "$MODE" = "plan" ] || [ "$HARD_FAILURES" -eq 0 ]; then
      exit 0
    fi
    exit 2
  fi
  [ "$HARD_FAILURES" -eq 0 ] || { say "preflight refused ($HARD_FAILURES hard failure(s))"; exit 2; }

  if [ "$SKIP_BUILD" -eq 0 ]; then
    build_stack || exit 3
  else
    head2 "build"
    say "skipped (--skip-build)"
  fi

  [ -x "$BIN_DIR/qualia" ] || { say "the CLI is not built at $BIN_DIR/qualia"; exit 3; }
  if ! start_stack; then
    stop_stack
    exit 4
  fi
  assert_mission
  if [ "$KEEP_STACK" -eq 1 ]; then
    say "leaving the stack running (--keep-stack); stop it with: $BIN_DIR/qualia stop --manifest $MANIFEST"
  else
    stop_stack
  fi
  summary
}

main
