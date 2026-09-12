#!/bin/sh
# Repeat the belief-coherence test on the board, one log per run.
#
# usage: run_repeats.sh <label> <runs>
#
# ROOT defaults to the board's archive tree; logs land in /tmp/fix100 and are
# copied into this directory's runs/ afterwards.
ROOT=${ROOT:-/home/jetson/remediate/515a506}
label=$1
runs=$2
export PATH="$HOME/.cargo/bin:$PATH"
mkdir -p /tmp/fix100
cd "$ROOT" || exit 1
pass=0
fail=0
i=1
while [ "$i" -le "$runs" ]; do
  RUSTFLAGS="-C target-feature=+fp16" cargo test -p qualia-arena-recorder --offline -j 2 \
    a_coherent_read_of_the_recorded_belief_is_never_torn > "/tmp/fix100/${label}_$i.log" 2>&1
  rc=$?
  if [ "$rc" -eq 0 ]; then
    pass=$((pass + 1))
  else
    fail=$((fail + 1))
  fi
  detail=$(grep -E "left:|right:|test result:" "/tmp/fix100/${label}_$i.log" | tr '\n' ' ')
  echo "$label run $i: EXIT=$rc $detail"
  i=$((i + 1))
done
echo "$label SUMMARY: pass=$pass fail=$fail"
