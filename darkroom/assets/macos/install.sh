#!/usr/bin/env bash
# Assemble Darkroom.app around the built binary and register the `.darkroom`
# association with Launch Services. Run after `cargo build --release`; installs
# no binary of its own — the bundle's executable is a symlink onto the build
# output, which `cargo clean` removes. Re-runnable.
#
# All users:  sudo APP_DIR=/Applications darkroom/assets/macos/install.sh
# Undo:       lsregister -u <bundle> && rm -rf <bundle>
set -euo pipefail
cd "$(dirname "$0")"

# Both must match Info.plist: the bundle id is what claims the type, and the
# UTI is what the claim names. Nothing else ties the two together.
APP_ID=com.cssodessa.darkroom
UTI=com.cssodessa.darkroom.project
APP_DIR="${APP_DIR:-$HOME/Applications}"
APP="$APP_DIR/Darkroom.app"
LSREGISTER=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister

[ -x "$LSREGISTER" ] || { echo "error: no lsregister — this is a macOS script" >&2; exit 1; }

# Prefer an installed binary over the build tree, as the Linux entry does. Both
# forms resolve to an absolute path here: a bundle names its executable by a
# file inside itself, so there is no bare-name form that survives the binary
# moving. `readlink -f` is GNU, hence the cd/pwd idiom.
BIN="${DARKROOM_BIN:-}"
[ -n "$BIN" ] || BIN=$(command -v darkroom || true)
if [ -z "$BIN" ]; then
  for candidate in ../../../target/{release,debug}/darkroom; do
    if [ -x "$candidate" ]; then
      BIN="$(cd "$(dirname "$candidate")" && pwd)/darkroom"
      break
    fi
  done
fi
# Fatal, where the Linux script only warns: an inert `.desktop` entry is
# ignored and the file falls back to the archive handler, but a bundle whose
# executable is missing still claims the type — every `.darkroom` file would
# become unopenable rather than merely unassociated.
if [ -z "$BIN" ]; then
  echo "error: no darkroom binary on PATH or in target/; run cargo build --release" >&2
  exit 1
fi

# A bundle that isn't ours is someone else's app of the same name — replacing
# it is not this script's business.
if [ -e "$APP" ]; then
  existing=$(plutil -extract CFBundleIdentifier raw -o - "$APP/Contents/Info.plist" 2>/dev/null || true)
  if [ "$existing" != "$APP_ID" ]; then
    echo "error: $APP already exists and is not $APP_ID (found '${existing:-none}')" >&2
    exit 1
  fi
  rm -rf "$APP"
fi

echo "installing into $APP (executable -> $BIN)"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp Info.plist "$APP/Contents/Info.plist"
cp ../icons/darkroom.icns "$APP/Contents/Resources/darkroom.icns"
# Matches CFBundlePackageType; the four `?` are the unset creator code.
printf 'APPL????' >"$APP/Contents/PkgInfo"
ln -sfn "$BIN" "$APP/Contents/MacOS/darkroom"

# Stamp the version the plist deliberately omits. Asking the binary rather than
# parsing Cargo.toml keeps the bundle describing what it actually wraps, and
# needs no cargo. A binary too broken to answer gets no version keys — they are
# descriptive, and failing the install over them would be worse.
VERSION=$("$APP/Contents/MacOS/darkroom" --version 2>/dev/null | awk 'NR==1{print $NF}')
if [ -n "$VERSION" ]; then
  plutil -replace CFBundleShortVersionString -string "$VERSION" "$APP/Contents/Info.plist"
  plutil -replace CFBundleVersion -string "$VERSION" "$APP/Contents/Info.plist"
else
  echo "warning: $BIN did not report a version; the bundle carries none" >&2
fi

# `-lint` reports plist problems that would otherwise register a bundle whose
# type claim silently does nothing.
"$LSREGISTER" -f -lint "$APP"

# The Info.plist's `LSHandlerRank: Owner` is what makes darkroom the default,
# and it settles the type unaided while nothing else claims it. duti only
# matters once something does — it is not part of a stock macOS.
if command -v duti >/dev/null; then
  duti -s "$APP_ID" "$UTI" all 2>/dev/null || true
fi

# Read back from the database rather than from what was just written, so an
# unregistrable bundle cannot look installed — the `gio mime` step's purpose in
# the Linux script.
dump=$("$LSREGISTER" -dump 2>/dev/null || true)
if ! grep -Fq "$APP" <<<"$dump"; then
  echo "warning: $APP never reached the database — is $BIN reachable?" >&2
elif ! grep -Fq "$UTI" <<<"$dump"; then
  echo "warning: $UTI is claimed by nothing — check UTExportedTypeDeclarations" >&2
else
  echo "done: .darkroom -> $APP"
fi

# The gap documented at the top of Info.plist, repeated where it will be read:
# without it the first double-click looks like this script failed.
echo "note: Finder will hand .darkroom files to Darkroom, but the app opens empty —"
echo "      macOS delivers the path as an Apple Event rather than in argv, and winit"
echo "      0.30 exposes no hook for it. Pass the path on the command line meanwhile."
