#!/usr/bin/env bash
# Build darkroom with the `release-max` profile and wrap it in a single-file
# Flatpak bundle. Unlike install.sh — which points the desktop entry at whatever
# was last compiled on this machine — this produces a movable darkroom.flatpak
# that carries the binary, its icons and the `.darkroom` association. Everything
# runs against the freedesktop runtime, so the result installs on any distro.
# Re-runnable.
#
#   darkroom/assets/linux/flatpak.sh
#   flatpak install --user darkroom.flatpak
#
# Environment:
#   BUNDLE_DIR   where the bundle is staged (default target/<profile>/bundle/linux)
#   PROFILE      cargo profile to build and bundle (default release-max)
#   BRANCH       flatpak branch to export under (default master)
#   RUNTIME_VER  freedesktop runtime/SDK version (default 25.08)
#   FINISH_ARGS  sandbox permissions, replacing the defaults below
#   SKIP_BUILD   non-empty reuses the existing binary instead of invoking cargo
#
# The compile happens *inside* the SDK sandbox rather than on the host: the
# runtime's glibc is older than a rolling distro's, so a host-built binary would
# resolve symbols at link time that the runtime cannot provide at load time.
# That also means the build needs the rust SDK extension, installed below if
# missing. Host ~/.cargo is shared in, so crates already downloaded are reused.
#
# Objects land in the ordinary target/<profile>, so the binary sits where a host
# `cargo build` would put it. The two toolchains differ in sysroot and libc, so
# alternating a host build with this one re-fingerprints the graph and rebuilds
# it — and whichever ran last owns target/<profile>/darkroom.
#
# The fat-LTO, one-codegen-unit release-max link is long and memory-hungry; the
# whole dependency graph goes through it in a single unit.
set -euo pipefail
cd "$(dirname "$0")"
ROOT=$(cd ../../.. && pwd)

[ "$(uname -s)" = Linux ] || { echo "error: this is a Linux script" >&2; exit 1; }
command -v flatpak >/dev/null || { echo "error: flatpak is not installed" >&2; exit 1; }

# Must match the desktop entry's basename and the window's Wayland app_id (set
# in `main.rs`) — flatpak exports only files named for the app id, and the shell
# pairs a window with its launcher only when the two agree.
APP_ID=com.cssodessa.darkroom
PROFILE="${PROFILE:-release-max}"
BRANCH="${BRANCH:-master}"
RUNTIME_VER="${RUNTIME_VER:-25.08}"
RUST_EXT=org.freedesktop.Sdk.Extension.rust-stable

# `--profile dev`/`test` build into target/debug; every other profile gets a
# directory of its own name.
case "$PROFILE" in
  dev | test) TARGET_DIR="$ROOT/target/debug" ;;
  *) TARGET_DIR="$ROOT/target/$PROFILE" ;;
esac
BIN="$TARGET_DIR/darkroom"

WORK="${BUNDLE_DIR:-$TARGET_DIR/bundle/linux}"
BUILD="$WORK/build"
REPO="$WORK/repo"
BUNDLE="$WORK/darkroom.flatpak"

missing=()
for ref in "org.freedesktop.Platform//$RUNTIME_VER" "org.freedesktop.Sdk//$RUNTIME_VER" \
  "$RUST_EXT//$RUNTIME_VER"; do
  flatpak info "$ref" >/dev/null 2>&1 || missing+=("$ref")
done
if [ ${#missing[@]} -gt 0 ]; then
  echo "installing ${missing[*]}"
  # Into the per-user installation, which needs no root — but that half of
  # flatpak has its own remote list, and a flathub configured system-wide is not
  # visible from it. `flatpak info` above already searched both, so anything
  # still missing here really is absent.
  flatpak remote-add --user --if-not-exists flathub https://flathub.org/repo/flathub.flatpakrepo
  flatpak install --user --noninteractive flathub "${missing[@]}"
fi

if [ -z "${SKIP_BUILD:-}" ]; then
  echo "building darkroom (--profile $PROFILE, inside $RUST_EXT//$RUNTIME_VER)"
  # lens turns on lumos/ml, so ort-sys downloads a prebuilt static onnxruntime
  # and emits an absolute -L into this cache. Nothing under it is reachable
  # unless the sandbox mounts it, and the link fails on the missing library
  # long after the build script reported success. Created first: --filesystem
  # silently skips a path that does not exist, which on a machine that has
  # never built ort would reintroduce exactly that failure.
  ORT_CACHE="${XDG_CACHE_HOME:-$HOME/.cache}/ort.pyke.io"
  mkdir -p "$ORT_CACHE"
  # A throwaway prefix: build-init below wants a directory it owns, and the
  # compile only needs the sandbox, not the app tree that gets assembled after.
  rm -rf "$WORK/sdk"
  # Under its own id, not $APP_ID: `flatpak build` names the systemd scope it
  # runs in after the id, and the shell reads a scope matching an installed
  # desktop entry as that app launching — so a compile under $APP_ID puts a
  # spurious darkroom in the panel for as long as cargo runs.
  flatpak build-init --sdk-extension="$RUST_EXT" \
    "$WORK/sdk" "$APP_ID.Build" org.freedesktop.Sdk org.freedesktop.Platform "$RUNTIME_VER"
  # cargo discovers .cargo/config.toml by walking up from the *working*
  # directory, not from --manifest-path, so the workspace's f16c rustflags only
  # apply if the sandbox starts here.
  (cd "$ROOT" && flatpak build \
    --share=network \
    --filesystem="$ROOT" \
    --filesystem="${CARGO_HOME:-$HOME/.cargo}" \
    --filesystem="$ORT_CACHE" \
    --env=PATH="/usr/lib/sdk/rust-stable/bin:/app/bin:/usr/bin" \
    --env=CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}" \
    "$WORK/sdk" \
    cargo build --profile "$PROFILE" -p darkroom)
fi

[ -x "$BIN" ] || { echo "error: no binary at $BIN" >&2; exit 1; }

# All that stands between a stray target/ and `rm -rf` on an unrelated tree.
if [ -e "$BUILD/metadata" ]; then
  grep -qx "name=$APP_ID" "$BUILD/metadata" ||
    { echo "error: $BUILD exists and is not $APP_ID" >&2; exit 1; }
fi
rm -rf "$BUILD"

echo "assembling $BUILD"
flatpak build-init "$BUILD" "$APP_ID" org.freedesktop.Sdk org.freedesktop.Platform "$RUNTIME_VER"
FILES="$BUILD/files"
install -Dm755 "$BIN" "$FILES/bin/darkroom"
for n in 16 24 32 48 64 128 256 512; do
  install -Dm644 "../icons/darkroom-$n.png" "$FILES/share/icons/hicolor/${n}x${n}/apps/$APP_ID.png"
done
# Exec stays bare: flatpak rewrites it at export into a `flatpak run` line, and
# /app/bin is on PATH inside the sandbox either way.
install -Dm644 "$APP_ID.desktop" "$FILES/share/applications/$APP_ID.desktop"

# install.sh gives the mime type its own icon by dropping one into the theme's
# `mimetypes` directories, but flatpak exports only files named for the app id,
# so those would be silently left behind. Naming the app's icon in the type
# declaration reaches the same place through a file that does get exported.
MIME_XML=$(sed "s|<sub-class-of|<icon name=\"$APP_ID\"/>\n    <sub-class-of|" darkroom-mime.xml)
case "$MIME_XML" in
  *"<icon name=\"$APP_ID\"/>"*) ;;
  *) echo "error: darkroom-mime.xml no longer has the anchor the icon is spliced onto" >&2; exit 1 ;;
esac
printf '%s\n' "$MIME_XML" | install -Dm644 /dev/stdin "$FILES/share/mime/packages/$APP_ID.xml"

# Ask the binary rather than Cargo.toml, so the line printed at the end
# describes what was packaged — and proves the copy runs under the runtime it
# will ship against, which the host's newer libc would not have shown.
VERSION=$(flatpak build "$BUILD" /app/bin/darkroom --version 2>/dev/null | awk 'NR==1{print $NF}') || true
[ -n "$VERSION" ] || echo "warning: the built binary reported no version" >&2

# Wayland with an X11 fallback (which needs ipc for MIT-SHM), the GPU for wgpu's
# vulkan backend, and $HOME for the documents and image files the editor opens.
# File dialogs and the light/dark preference go through portals, which need no
# permission of their own.
read -ra finish <<<"${FINISH_ARGS:---socket=wayland --socket=fallback-x11 --share=ipc --device=dri --filesystem=home}"
flatpak build-finish "$BUILD" --command=darkroom "${finish[@]}"

echo "exporting to $REPO"
flatpak build-export "$REPO" "$BUILD" "$BRANCH"
mkdir -p "$(dirname "$BUNDLE")"
# Without the runtime repo the bundle installs only where the freedesktop
# runtime already is; with it, flatpak knows where to fetch the missing half.
flatpak build-bundle --runtime-repo=https://flathub.org/repo/flathub.flatpakrepo \
  "$REPO" "$BUNDLE" "$APP_ID" "$BRANCH"

echo "done: $BUNDLE ($(du -h "$BUNDLE" | awk '{print $1}'), version ${VERSION:-unknown})"
echo "      flatpak install --user $BUNDLE"

# wgpu reaches the GPU through the runtime's driver extension, which flatpak
# picks by exact host driver version — a mismatch drops it to llvmpipe or fails
# outright, long after this script looked like it succeeded.
if [ -r /sys/module/nvidia/version ]; then
  GL="org.freedesktop.Platform.GL.nvidia-$(tr . - </sys/module/nvidia/version)"
  flatpak info "$GL" >/dev/null 2>&1 ||
    echo "note: host nvidia driver has no runtime counterpart — flatpak install --user flathub $GL" >&2
fi
