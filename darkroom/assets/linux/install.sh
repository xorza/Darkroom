#!/usr/bin/env bash
# Wire darkroom into the desktop: launcher icons, the menu entry, and the
# `.darkroom` file association. Run after `cargo build --release`; installs no
# binary of its own. Re-runnable.
#
# System-wide:  PREFIX=/usr/local sudo darkroom/assets/linux/install.sh
set -euo pipefail
cd "$(dirname "$0")"

# The entry's basename, the icon name, and the window's Wayland app_id / X11
# WM_CLASS (set in `main.rs`) are all this one string. The shell pairs a window
# with its launcher only when they agree.
APP_ID=com.cssodessa.darkroom
MIME=application/x-darkroom
DATA="${PREFIX:+$PREFIX/share}"
DATA="${DATA:-$HOME/.local/share}"

# GIO discards an entry whose Exec binary it cannot resolve, and silently — the
# association then looks installed while file managers fall back to whatever
# handles the parent `application/zip`. Prefer PATH; else bake in the build
# output, which `cargo clean` removes.
BIN="${DARKROOM_BIN:-}"
if [ -z "$BIN" ] && command -v darkroom >/dev/null; then
  BIN=darkroom # bare, so the entry survives the binary being moved
fi
if [ -z "$BIN" ]; then
  for candidate in ../../../target/{release,debug}/darkroom; do
    if [ -x "$candidate" ]; then
      BIN=$(readlink -f "$candidate")
      break
    fi
  done
fi
if [ -z "$BIN" ]; then
  echo "warning: no darkroom binary on PATH or in target/; the entry stays inert" >&2
  BIN=darkroom
fi

echo "installing into $DATA (Exec=$BIN)"
for n in 16 24 32 48 64 128 256 512; do
  install -Dm644 "../icons/darkroom-$n.png" "$DATA/icons/hicolor/${n}x${n}/apps/$APP_ID.png"
  install -Dm644 "../icons/darkroom-$n.png" \
    "$DATA/icons/hicolor/${n}x${n}/mimetypes/${MIME/\//-}.png"
done
install -Dm644 darkroom-mime.xml "$DATA/mime/packages/darkroom.xml"
sed "s|^Exec=darkroom |Exec=$BIN |" "$APP_ID.desktop" |
  install -Dm644 /dev/stdin "$DATA/applications/$APP_ID.desktop"

gtk-update-icon-cache -f -t "$DATA/icons/hicolor" 2>/dev/null || true
update-mime-database "$DATA/mime" 2>/dev/null || true
update-desktop-database "$DATA/applications" 2>/dev/null || true
# Per-user only: this writes the caller's mimeapps.list, which a system-wide
# install has no business touching.
[ -n "${PREFIX:-}" ] || xdg-mime default "$APP_ID.desktop" "$MIME" 2>/dev/null || true

# `xdg-mime query default` only reads back the line just written, so it cannot
# catch an unloadable entry; `gio mime` resolves it the way a file manager does.
if command -v gio >/dev/null && ! gio mime "$MIME" | grep -q "$APP_ID.desktop"; then
  echo "warning: $MIME resolves to nothing — is $BIN reachable?" >&2
else
  echo "done: $MIME -> $APP_ID.desktop"
fi
