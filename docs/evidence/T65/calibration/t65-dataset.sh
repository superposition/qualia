#!/usr/bin/env bash
# T65 (#250) board leg: build the dataset from the calibrated real session and
# let the trainer's preflight run past calibration.
#
#   t65-dataset.sh <session-name> <environment-id> <condition> <entity>
set -u
export PATH="$HOME/.cargo/bin:/usr/local/cuda/bin:$PATH"
BIN="$HOME/qualia-deploy/T65/target/release"
ROOT="$HOME/t65-capture"
SESSION="${1:?session}"
ENVIRONMENT="${2:-pinkie-bench}"
CONDITION="${3:-bench-static}"
ENTITY="${4:-pinkie}"
RUN="$ROOT/$SESSION"
MCAP=$(ls "$RUN"/mcap/*.mcap 2>/dev/null | head -1)
if [ -z "$MCAP" ]; then echo "NO_MCAP in $RUN/mcap"; exit 9; fi
SHA=$(sha256sum "$MCAP" | cut -d' ' -f1)
echo "MCAP=$MCAP"
echo "SHA256=$SHA"

cat > "$RUN/catalog.json" <<EOF
{
  "sources": [
    {
      "session_id": "$SESSION",
      "environment_id": "$ENVIRONMENT",
      "condition": "$CONDITION",
      "primary_entity": "$ENTITY",
      "path": "$MCAP",
      "sha256": "$SHA"
    }
  ]
}
EOF
echo "== catalog =="
cat "$RUN/catalog.json"

echo "== qualia-jepa-dataset =="
rm -rf "$RUN/dataset"; mkdir -p "$RUN/dataset"
"$BIN/qualia-jepa-dataset" --catalog "$RUN/catalog.json" --output-dir "$RUN/dataset" 2>&1 | tee "$RUN/dataset.log"
echo "DATASET_RC=${PIPESTATUS[0]}"

MAN=$(ls "$RUN"/dataset/*.json 2>/dev/null | head -1)
echo "MANIFEST=$MAN"
if [ -n "$MAN" ]; then
  cp "$MAN" "$RUN/dataset-manifest.json"
  echo "== manifest audit =="
  grep -E '"schema_version"|"digest"|"valid_transitions"|"candidate_transitions"|"sessions"|"environments"|"conditions"|"rejected"|"sample_count"|"mean_sensor_skew_ns"|"mean_action_coverage"' "$RUN/dataset-manifest.json" | head -20
  echo "== qualia-jepa-train (one epoch; the expectation is a refusal past calibration) =="
  rm -rf "$RUN/train"; mkdir -p "$RUN/train"
  "$BIN/qualia-jepa-train" --manifest "$MAN" --checkpoint-id t65-real-e1 \
      --output-dir "$RUN/train" --epochs 1 --batch-size 32 --seed 65 2>&1 | tee "$RUN/train.log"
  echo "TRAIN_RC=${PIPESTATUS[0]}"
  ls -l "$RUN/train"
fi
echo "== end =="
