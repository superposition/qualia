#!/usr/bin/env bash
# Launch the native Studio application, building it first if needed.
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
BUILD_DIR="${QUALIA_STUDIO_BUILD_DIR:-$ROOT/studio/build}"

MODE="${1:-studio}"
if [[ "$MODE" == "reconstruct" ]]; then
  shift
  TARGET="qualia-reconstruction-studio"
  APP="$BUILD_DIR/qualia-reconstruction-studio.app/Contents/MacOS/qualia-reconstruction-studio"
  LABEL="Reconstruction Studio"
else
  TARGET="qualia-studio"
  APP="$BUILD_DIR/qualia-studio.app/Contents/MacOS/qualia-studio"
  LABEL="Studio"
fi

if [[ ! -x "$APP" ]]; then
  echo "qualia: building $LABEL..."
  cmake -S "$ROOT/studio" -B "$BUILD_DIR"
  cmake --build "$BUILD_DIR" --target "$TARGET" -j 4
fi

echo "qualia: launching $LABEL"
exec "$APP" "$@"
