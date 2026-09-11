#!/usr/bin/env bash
# Thin wrapper so CI and Make targets can call the check without knowing the
# interpreter. The logic lives in provenance_check.py.
set -euo pipefail
exec python3 "$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/provenance_check.py" "$@"
