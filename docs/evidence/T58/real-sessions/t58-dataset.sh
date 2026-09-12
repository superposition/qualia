#!/usr/bin/env bash
# T58 (#239) board leg: build the JEPA dataset from the real capture and try to train.
#
#   t58-dataset.sh <session-name> <environment-id> <condition> <entity>
set -u
export PATH="$HOME/.cargo/bin:/usr/local/cuda/bin:$PATH"
BIN="$HOME/qualia-deploy/T58c/target/release"
ROOT="$HOME/t58-capture"
SESSION="${1:?session}"
ENVIRONMENT="${2:-pinkie-bench}"
CONDITION="${3:-indoor-static}"
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
  python3 - "$MAN" <<'PY'
import json,sys
m=json.load(open(sys.argv[1]))
a=m["audit"]
print("schema", m["schema_version"])
print("digest", m["digest"])
print("sources", len(m["sources"]), "samples", len(m["samples"]))
print("valid_transitions", a["valid_transitions"], "candidate_transitions", a["candidate_transitions"])
print("sessions", a["sessions"], "environments", a["environments"], "conditions", a["conditions"])
print("rejected", json.dumps(a["rejected"], sort_keys=True))
print("split_samples", json.dumps(a["split_samples"], sort_keys=True))
print("mean_sensor_skew_ns", a["mean_sensor_skew_ns"], "p95_sensor_skew_ns", a["p95_sensor_skew_ns"])
print("mean_action_coverage", a["mean_action_coverage"], "overexposed_candidate_fraction", a["overexposed_candidate_fraction"])
PY
  echo "== qualia-jepa-train (bounded, one epoch) =="
  rm -rf "$RUN/train"; mkdir -p "$RUN/train"
  "$BIN/qualia-jepa-train" --manifest "$MAN" --checkpoint-id t58-real-e1 \
      --output-dir "$RUN/train" --backend cuda --epochs 1 --batch-size 32 --seed 58 2>&1 | tee "$RUN/train.log"
  echo "TRAIN_RC=${PIPESTATUS[0]}"
  ls -l "$RUN/train"
fi
echo "== end =="
