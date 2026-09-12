#!/bin/sh
# Repeat the belief-coherence test on the board, one log per run.
#
# usage: run_repeats.sh <label> <runs>
#
# Every run's log carries its own provenance header before cargo's output:
# board-local start and end (ISO 8601 with offset), the 1-minute load average at
# the moment the run starts, and the number of cargo/rustc processes visible —
# so a reader can tell a quiet run from one taken under a build without a braid.
# The per-run lines are also written to <label>_summary.log.
#
# ROOT defaults to the board's archive tree; logs land in /tmp/fix100 and are
# copied into this directory's runs/ afterwards.
ROOT=${ROOT:-/home/jetson/remediate/515a506}
label=$1
runs=$2
export PATH="$HOME/.cargo/bin:$PATH"
mkdir -p /tmp/fix100
cd "$ROOT" || exit 1
summary="/tmp/fix100/${label}_summary.log"
: > "$summary"
pass=0
fail=0
i=1
while [ "$i" -le "$runs" ]; do
  log="/tmp/fix100/${label}_$i.log"
  start=$(date '+%Y-%m-%dT%H:%M:%S%z')
  load=$(cut -d' ' -f1 /proc/loadavg)
  busy=$(ps -eo comm= | grep -cE '^(cargo|rustc)$' || true)
  printf '# board-local start %s  load1 %s  cargo/rustc %s  %s run %s\n' \
    "$start" "$load" "$busy" "$label" "$i" > "$log"
  RUSTFLAGS="-C target-feature=+fp16" cargo test -p qualia-arena-recorder --offline -j 2 \
    a_coherent_read_of_the_recorded_belief_is_never_torn >> "$log" 2>&1
  rc=$?
  end=$(date '+%Y-%m-%dT%H:%M:%S%z')
  printf '# board-local end %s  load1 %s  exit %s\n' \
    "$end" "$(cut -d' ' -f1 /proc/loadavg)" "$rc" >> "$log"
  if [ "$rc" -eq 0 ]; then
    pass=$((pass + 1))
  else
    fail=$((fail + 1))
  fi
  detail=$(grep -E "left:|right:|test result:" "$log" | tr '\n' ' ')
  line="$label run $i: start=$start end=$end EXIT=$rc $detail"
  echo "$line" | tee -a "$summary"
  i=$((i + 1))
done
echo "$label SUMMARY: pass=$pass fail=$fail" | tee -a "$summary"
