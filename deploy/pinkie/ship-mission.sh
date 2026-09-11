#!/usr/bin/env bash
# T29 host side — stage the head as a `git archive` and ship it to Pinkie.
#
# The board has no DNS (D-010): `git clone` and crates.io are unreachable there,
# so the tree travels as a tarball. It travels as `git archive` output rather
# than a working copy because a Windows checkout writes CRLF into the files, and
# CRLF breaks shell scripts and the cross-build Dockerfile on Linux (D-010). The
# archive is the commit's blobs, so `deploy/pinkie/run-mission.sh` on the board
# runs what this repository actually committed.
#
# The dev host cannot cross-build for the board today (D-018: Docker engine
# unreachable, `cross` not installed, no aarch64 gcc), so the board builds
# natively from this archive — the lane D-018 names as working.
#
# Usage:
#   bash deploy/pinkie/ship-mission.sh --stage-only      # write the tarballs, print the next commands
#   bash deploy/pinkie/ship-mission.sh                   # stage, then scp to the board
#
# Options:
#   --rev REV        the revision to ship (default: HEAD)
#   --out DIR        where the tarballs are written (default: ~/qualia-board-ship)
#   --prior DIR      a prior directory outside the tree to ship alongside (default: none)
#   --board-dir DIR  the board's landing directory (default: ~/qualia-board)
#   --board HOST     the board's ssh target (default: jetson@192.168.55.1)
#   --identity FILE  the ssh key (default: ~/.ssh/qualia_jetson_ed25519)
#   --stage-only     never touch the network
#
# Exit codes: 0 the tarball is staged (and shipped); 1 staging failed; 2 a usage error.

set -euo pipefail

REV="HEAD"
OUT=""
PRIOR=""
BOARD_DIR="~/qualia-board"
BOARD="jetson@192.168.55.1"
IDENTITY="${QUALIA_BOARD_KEY:-$HOME/.ssh/qualia_jetson_ed25519}"
STAGE_ONLY=0
REPO=""

while [ $# -gt 0 ]; do
  case "$1" in
    --repo) REPO="$2"; shift 2 ;;
    --rev) REV="$2"; shift 2 ;;
    --out) OUT="$2"; shift 2 ;;
    --prior) PRIOR="$2"; shift 2 ;;
    --board-dir) BOARD_DIR="$2"; shift 2 ;;
    --board) BOARD="$2"; shift 2 ;;
    --identity) IDENTITY="$2"; shift 2 ;;
    --stage-only) STAGE_ONLY=1; shift ;;
    -h|--help) sed -n '2,34p' "$0"; exit 0 ;;
    *) echo "ship-mission: unknown option: $1" >&2; exit 2 ;;
  esac
done

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
: "${REPO:=$(cd -- "$SCRIPT_DIR/../.." && pwd)}"
: "${OUT:=$HOME/qualia-board-ship}"

git -C "$REPO" rev-parse --verify --quiet "$REV^{commit}" >/dev/null || {
  echo "ship-mission: $REV is not a commit in $REPO" >&2
  exit 2
}
SHA="$(git -C "$REPO" rev-parse "$REV^{commit}")"
SHORT="$(git -C "$REPO" rev-parse --short=12 "$REV^{commit}")"
TARBALL="$OUT/qualia-$SHORT.tar.gz"

mkdir -p "$OUT"

if [ -n "$(git -C "$REPO" status --porcelain)" ]; then
  echo "ship-mission: warning - the worktree is dirty; the archive is $SHA's committed content, not the worktree" >&2
fi

echo "commit:   $SHA"
echo "short:    $SHORT"
echo "tarball:  $TARBALL"

# `git archive` is the committed blobs; gzip is deterministic enough to hash.
if ! git -C "$REPO" archive --format=tar --prefix=qualia/ "$SHA" | gzip -9 >"$TARBALL"; then
  echo "ship-mission: git archive failed" >&2
  exit 1
fi

# LF, not CRLF: the reason the archive travels instead of a working copy (D-010).
# What Linux executes or parses with make is refused outright — a CRLF shell
# script or Makefile is broken on the board, and D-010 records exactly that. The
# rest of the shipped text is reported, not refused: a committed CRLF Markdown or
# Mermaid source changes nothing the board runs or parses.
#
# `|| true` on both listings: `grep` exits 1 when the archive holds no member of
# that kind, and a bare command substitution under `set -e` would abort the ship
# with no message. A member's CR is found by the shell's own pattern match on the
# extracted bytes — `grep -q $'\r'` does not match a CR under this workstation's
# MINGW grep, which would make the guard silently blind exactly where it matters.
EXEC_MEMBERS="$(tar tzf "$TARBALL" | grep -E '\.(sh|py)$|(^|/)(Dockerfile|Makefile)[^/]*$' || true)"
TEXT_MEMBERS="$(tar tzf "$TARBALL" | grep -E '\.(toml|mmd|ts|mjs|html|css|ya?ml|json)$' || true)"

scan_members() {
  # scan_members <members> <offenders-var> <count-var>
  local member content count=0 offenders=""
  while IFS= read -r member; do
    [ -n "$member" ] || continue
    count=$((count + 1))
    content="$(tar xOzf "$TARBALL" "$member" 2>/dev/null)" || continue
    case "$content" in
      *$'\r'*) offenders="${offenders}${member}
" ;;
    esac
  done <<<"$1"
  printf -v "$2" '%s' "$offenders"
  printf -v "$3" '%s' "$count"
}

CR_EXEC=""
EXEC_COUNT=0
scan_members "$EXEC_MEMBERS" CR_EXEC EXEC_COUNT
if [ -n "$CR_EXEC" ]; then
  echo "ship-mission: the archive carries CRLF in a file Linux runs; do not ship it:" >&2
  printf '%s' "$CR_EXEC" >&2
  exit 1
fi
echo "check:    no CR in $EXEC_COUNT shipped script, Makefile and Dockerfile(s) (the archive is LF)"

CR_TEXT=""
TEXT_COUNT=0
scan_members "$TEXT_MEMBERS" CR_TEXT TEXT_COUNT
if [ -n "$CR_TEXT" ]; then
  echo "note:     $(printf '%s' "$CR_TEXT" | wc -l) other shipped file(s) carry CRLF (the parsers the board uses accept them; a shell or make would not):"
  printf '%s' "$CR_TEXT" | sed 's/^/          /'
fi

SIZE="$(wc -c <"$TARBALL")"
echo "bytes:    $SIZE"
echo "sha256:   $(sha256sum "$TARBALL" | cut -d' ' -f1)"
echo "files:    $(tar tzf "$TARBALL" | grep -vc '/$' || true)"

PRIOR_NOTE=""
REPO_ABS="$(cd -- "$REPO" && pwd)"
if [ -n "$PRIOR" ]; then
  PRIOR_ABS="$(cd -- "$PRIOR" && pwd)"
  case "$PRIOR_ABS" in
    "$REPO_ABS"/*)
      PRIOR_NOTE="the prior ships inside the archive at ${PRIOR_ABS#"$REPO_ABS"/}"
      ;;
    *)
      PRIOR_TARBALL="$OUT/qualia-prior-$SHORT.tar.gz"
      tar czf "$PRIOR_TARBALL" -C "$(dirname -- "$PRIOR_ABS")" "$(basename -- "$PRIOR_ABS")"
      PRIOR_NOTE="the prior ships separately at $PRIOR_TARBALL"
      ;;
  esac
  echo "prior:    $PRIOR_ABS ($PRIOR_NOTE)"
fi

cat <<EOF

board invocation:
  scp -i $IDENTITY $TARBALL $BOARD:$BOARD_DIR/
  ssh -i $IDENTITY $BOARD
    mkdir -p $BOARD_DIR/$SHORT $BOARD_DIR/runs
    tar xzf $BOARD_DIR/qualia-$SHORT.tar.gz -C $BOARD_DIR/$SHORT --strip-components=1
    cd $BOARD_DIR/$SHORT
    bash deploy/pinkie/run-mission.sh

run-mission.sh's defaults are already the ticket's: the zero-motion manifest,
assets/brain/prior, QUALIA_CUDA_SM=87, and the agent on 8081 (the board's 8000 is
Leash, 8080 llama.cpp). It prints its plan first if you want to read the exact
commands before any of them run: --plan.
EOF

if [ "$STAGE_ONLY" -eq 1 ]; then
  echo ""
  echo "ship-mission: staged; nothing was sent (--stage-only)"
  exit 0
fi

scp -i "$IDENTITY" "$TARBALL" "$BOARD:$BOARD_DIR/" || {
  echo "ship-mission: scp failed; the board is at 192.168.55.1 over the USB gadget link (D-010)" >&2
  exit 1
}
if [ -n "${PRIOR_TARBALL:-}" ]; then
  scp -i "$IDENTITY" "$PRIOR_TARBALL" "$BOARD:$BOARD_DIR/" || {
    echo "ship-mission: scp of the prior tarball failed" >&2
    exit 1
  }
fi
echo ""
echo "ship-mission: shipped $SHORT to $BOARD:$BOARD_DIR/"
