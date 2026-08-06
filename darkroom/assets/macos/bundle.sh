#!/usr/bin/env bash
# Build darkroom with the `release-max` profile and assemble a self-contained
# Darkroom.app around the result. Unlike install.sh — which symlinks a dev build
# into ~/Applications so the `.darkroom` association points at whatever was last
# compiled — this copies the binary in, so the bundle is a movable artifact.
# Re-runnable.
#
#   darkroom/assets/macos/bundle.sh
#
# Environment:
#   BUNDLE_DIR   where Darkroom.app is written (default target/<profile>/bundle/macos)
#   PROFILE      cargo profile to build and bundle (default release-max)
#   CODESIGN_ID  signing identity; `-` is ad-hoc (default), empty skips signing
#   SKIP_BUILD   non-empty reuses the existing binary instead of invoking cargo
#
# The fat-LTO, one-codegen-unit release-max link is long and memory-hungry; the
# whole dependency graph goes through it in a single unit.
set -euo pipefail
cd "$(dirname "$0")"
ROOT=$(cd ../../.. && pwd)

[ "$(uname -s)" = Darwin ] || { echo "error: this is a macOS script" >&2; exit 1; }

# Must match Info.plist's CFBundleIdentifier — the check below is all that
# stands between a stray BUNDLE_DIR and `rm -rf` on someone else's bundle.
APP_ID=com.cssodessa.darkroom
PROFILE="${PROFILE:-release-max}"
# `--profile dev`/`test` build into target/debug; every other profile gets a
# directory of its own name.
case "$PROFILE" in
  dev | test) TARGET_DIR="$ROOT/target/debug" ;;
  *) TARGET_DIR="$ROOT/target/$PROFILE" ;;
esac
APP="${BUNDLE_DIR:-$TARGET_DIR/bundle/macos}/Darkroom.app"

if [ -z "${SKIP_BUILD:-}" ]; then
  echo "building darkroom (--profile $PROFILE)"
  (cd "$ROOT" && cargo build --profile "$PROFILE" -p darkroom)
fi

BIN="$TARGET_DIR/darkroom"
[ -x "$BIN" ] || { echo "error: no binary at $BIN" >&2; exit 1; }

if [ -e "$APP" ]; then
  found=$(plutil -extract CFBundleIdentifier raw -o - "$APP/Contents/Info.plist" 2>/dev/null || true)
  [ "$found" = "$APP_ID" ] || { echo "error: $APP exists and is '${found:-unknown}', not $APP_ID" >&2; exit 1; }
  rm -rf "$APP"
fi

echo "assembling $APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp Info.plist "$APP/Contents/Info.plist"
cp ../icons/darkroom.icns "$APP/Contents/Resources/darkroom.icns"
printf 'APPL????' >"$APP/Contents/PkgInfo" # type code + unset creator code
cp "$BIN" "$APP/Contents/MacOS/darkroom"

# Stamp the version Info.plist omits, asking the binary rather than Cargo.toml
# so the bundle describes what it wraps. Also proves the copy runs.
VERSION=$("$APP/Contents/MacOS/darkroom" --version 2>/dev/null | awk 'NR==1{print $NF}')
if [ -n "$VERSION" ]; then
  plutil -replace CFBundleShortVersionString -string "$VERSION" "$APP/Contents/Info.plist"
  plutil -replace CFBundleVersion -string "$VERSION" "$APP/Contents/Info.plist"
else
  echo "warning: the built binary reported no version" >&2
fi

# The signature covers Info.plist, so this has to follow the stamping. arm64
# binaries are ad-hoc signed by the linker already, but that signature is on the
# executable alone — signing the bundle is what produces _CodeSignature and
# makes the plist and icon tamper-evident.
ID="${CODESIGN_ID--}"
if [ -z "$ID" ]; then
  NOTE="note: signing was skipped, so this bundle carries only the linker's
      ad-hoc signature on the executable — Gatekeeper blocks it elsewhere."
elif [ "$ID" = "-" ]; then
  echo "signing ad-hoc"
  codesign --force --sign - "$APP"
  NOTE="note: an ad-hoc signature is fine locally, but Gatekeeper blocks the
      bundle on another Mac — set CODESIGN_ID to a Developer ID to ship it."
else
  echo "signing with identity '$ID'"
  # Hardened runtime and a trusted timestamp are what notarisation requires;
  # neither is available to an ad-hoc signature.
  codesign --force --options runtime --timestamp --sign "$ID" "$APP"
  NOTE="note: signed for notarisation — submit with \`xcrun notarytool\`, then
      \`xcrun stapler staple\`, before handing the bundle to anyone."
fi
[ -z "$ID" ] || codesign --verify --strict "$APP"

# Catches a plist that parses but says nothing usable — a bundle Launch Services
# would reject after the build already looked like it succeeded.
plutil -lint -s "$APP/Contents/Info.plist"

echo "done: $APP ($(du -sh "$APP" | awk '{print $1}'), version ${VERSION:-unknown})"
echo "$NOTE"
