#!/bin/sh
# Repeat one integration test on the board, keeping every run's log.
#
# usage: run_repeats.sh <label> <runs> <package> <test-target> <test-name>
#
# ROOT defaults to the board's archive tree. Logs land in /tmp/fix100 and are
# copied into this directory's runs/ afterwards.
ROOT=${ROOT:-/home/jetson/remediate/515a506}
label=$1
runs=$2
package=$3
target=$4
name=$5
export PATH="$HOME/.cargo/bin:$PATH"
mkdir -p /tmp/fix100
cd "$ROOT" || exit 1
pass=0
fail=0
i=1
while [ "$i" -le "$runs" ]; do
  # The agent's scratch region is 64 MiB and its libtest process exits without
  # dropping it, so repeated runs fill /dev/shm (1.8 GiB on Pinkie: the 20th
  # run dies with SIGBUS). Set CLEAN_SHM=1 to unlink the scratch regions
  # between runs.
  if [ "${CLEAN_SHM:-0}" = "1" ]; then
    rm -f /dev/shm/qualia-agent-test-*
  fi
  RUSTFLAGS="-C target-feature=+fp16" cargo test -p "$package" --offline -j 2 \
    --test "$target" "$name" > "/tmp/fix100/${label}_$i.log" 2>&1
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
