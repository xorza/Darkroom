#!/usr/bin/env bash
# Assemble Darkroom.app around the built binary and register the `.darkroom`
# association. Run after `cargo build --release`; the bundle's executable is a
# symlink onto the build output, so no binary is installed. Re-runnable.
#
# All users:  sudo APP_DIR=/Applications darkroom/assets/macos/install.sh
# Undo:       lsregister -u <bundle> && rm -rf <bundle>
set -euo pipefail
cd "$(dirname "$0")"

# APP_ID and UTI must match Info.plist — nothing else ties the two together.
APP_ID=com.cssodessa.darkroom
UTI=com.cssodessa.darkroom.project
APP="${APP_DIR:-$HOME/Applications}/Darkroom.app"
LSREGISTER=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister

[ -x "$LSREGISTER" ] || { echo "error: no lsregister — this is a macOS script" >&2; exit 1; }

# Absolute either way: a bundle names its executable by a file inside itself,
# so there is no bare-name form. (`readlink -f` is GNU, hence cd/pwd.)
BIN="${DARKROOM_BIN:-$(command -v darkroom || true)}"
if [ -z "$BIN" ]; then
  for candidate in ../../../target/{release,debug}/darkroom; do
    if [ -x "$candidate" ]; then
      BIN="$(cd "$(dirname "$candidate")" && pwd)/darkroom"
      break
    fi
  done
fi
# Fatal, where the Linux script only warns: a bundle with a missing executable
# still claims the type, making every `.darkroom` file unopenable.
if [ -z "$BIN" ]; then
  echo "error: no darkroom binary on PATH or in target/; run cargo build --release" >&2
  exit 1
fi

# Re-runnable means rm -rf, so make sure it is ours before removing it.
if [ -e "$APP" ]; then
  found=$(plutil -extract CFBundleIdentifier raw -o - "$APP/Contents/Info.plist" 2>/dev/null || true)
  [ "$found" = "$APP_ID" ] || { echo "error: $APP exists and is '${found:-unknown}', not $APP_ID" >&2; exit 1; }
  rm -rf "$APP"
fi

echo "installing $APP -> $BIN"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp Info.plist "$APP/Contents/Info.plist"
cp ../icons/darkroom.icns "$APP/Contents/Resources/darkroom.icns"
printf 'APPL????' >"$APP/Contents/PkgInfo" # type code + unset creator code
ln -sfn "$BIN" "$APP/Contents/MacOS/darkroom"

# Stamp the version Info.plist omits, asking the binary rather than Cargo.toml
# so the bundle describes what it wraps. Also proves the symlink runs.
VERSION=$("$APP/Contents/MacOS/darkroom" --version 2>/dev/null | awk 'NR==1{print $NF}')
if [ -n "$VERSION" ]; then
  plutil -replace CFBundleShortVersionString -string "$VERSION" "$APP/Contents/Info.plist"
  plutil -replace CFBundleVersion -string "$VERSION" "$APP/Contents/Info.plist"
else
  echo "warning: $BIN reported no version" >&2
fi

# `-lint` catches plist errors that would register a bundle whose claim does
# nothing. `duti` only matters once something else claims the type; without it
# Info.plist's `LSHandlerRank: Owner` settles it.
"$LSREGISTER" -f -lint "$APP"
command -v duti >/dev/null && duti -s "$APP_ID" "$UTI" all 2>/dev/null || true

# Read the database back, so an unregistrable bundle cannot look installed.
dump=$("$LSREGISTER" -dump 2>/dev/null || true)
if ! grep -Fq "$APP" <<<"$dump"; then
  echo "warning: $APP never reached the database — is $BIN reachable?" >&2
elif ! grep -Fq "$UTI" <<<"$dump"; then
  echo "warning: nothing claims $UTI — check UTExportedTypeDeclarations" >&2
else
  echo "done: .darkroom -> $APP"
fi

# Without this the first double-click reads as the install having failed.
echo "note: Finder routes .darkroom here, but the app opens empty — the path"
echo "      arrives as an Apple Event winit swallows (see Info.plist). Pass it"
echo "      on the command line meanwhile."
