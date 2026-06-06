#!/usr/bin/env bash
#
# fetch-test-data.sh — download the lumos test-data bundle and unpack it into
# test_data/ at the crate root. The bundle holds real RAW/FITS frames and other
# fixtures too large to commit; test_data/ is gitignored.
#
# Source: https://cssodessa.com/lumos_data.7z (password-protected 7z archive)
#
# Requires a 7-Zip binary (7zz, 7z, or 7za) on PATH: brew install sevenzip
#
# Usage:
#   scripts/fetch-test-data.sh           # fetch + unpack (skips if already present)
#   scripts/fetch-test-data.sh --force   # re-download and overwrite test_data/
#
# Idempotent: an already-populated test_data/ is left untouched unless --force.

set -euo pipefail

URL="https://cssodessa.com/lumos_data.7z"
PASSWORD='mv4Vs4Q58{(1'
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="$ROOT/test_data"
ARCHIVE="$ROOT/.tmp/lumos_data.7z"

FORCE=0
case "${1:-}" in
  --force) FORCE=1 ;;
  "")      ;;
  *) echo "unknown option: $1" >&2; exit 2 ;;
esac

if [ -d "$DEST" ] && [ -n "$(ls -A "$DEST" 2>/dev/null)" ]; then
  if [ "$FORCE" -eq 0 ]; then
    echo "test_data/ already populated — nothing to do (use --force to refresh)."
    exit 0
  fi
  echo "--force: clearing existing test_data/"
  rm -rf "$DEST"
fi

SEVENZIP=""
for bin in 7zz 7z 7za; do
  if command -v "$bin" >/dev/null 2>&1; then SEVENZIP="$bin"; break; fi
done
if [ -z "$SEVENZIP" ]; then
  echo "error: no 7-Zip binary found (looked for 7zz, 7z, 7za)." >&2
  echo "       install it with: brew install sevenzip" >&2
  exit 1
fi

mkdir -p "$(dirname "$ARCHIVE")" "$DEST"

echo "Downloading $URL"
curl -fL --progress-bar -o "$ARCHIVE.part" "$URL"
mv -f "$ARCHIVE.part" "$ARCHIVE"

echo "Unpacking into $DEST"
"$SEVENZIP" x -y -p"$PASSWORD" -o"$DEST" "$ARCHIVE"

echo "Done. test_data/ populated from $(basename "$URL")."
