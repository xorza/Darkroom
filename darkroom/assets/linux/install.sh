#!/usr/bin/env bash
# Install darkroom's icons, desktop entry and MIME type into the per-user XDG
# data dirs, so GNOME/KDE/etc. show the icon in launchers and the taskbar and
# open `.darkroom` files with it. Run after
# `cargo build --release` (this only wires up the desktop integration; it
# does not copy the binary). Re-runnable.
#
#   darkroom/assets/linux/install.sh
#
# System-wide instead of per-user:  PREFIX=/usr/local sudo darkroom/assets/linux/install.sh
set -euo pipefail
cd "$(dirname "$0")"
ICONS=../icons

DATA="${PREFIX:+$PREFIX/share}"
DATA="${DATA:-$HOME/.local/share}"

# Resolve what the entry's `Exec=` will name. This is load-bearing, not a
# nicety: GIO discards a desktop entry whose Exec binary it cannot find on
# PATH, and it does so silently — the file stays installed, `xdg-mime query
# default` still answers `darkroom.desktop`, and yet file managers fall back
# to whatever handles the parent `application/zip` type (Ark, on KDE). So an
# association that looks installed does nothing.
#
# A plain `darkroom` on PATH is preferred, since the entry stays valid however
# the binary is later moved. Failing that, the cargo build output is baked in
# absolutely — which works, but breaks on `cargo clean`.
BIN="${DARKROOM_BIN:-}"
if [ -z "$BIN" ] && command -v darkroom >/dev/null 2>&1; then
  BIN=darkroom
fi
if [ -z "$BIN" ]; then
  for candidate in ../../../target/release/darkroom ../../../target/debug/darkroom; do
    if [ -x "$candidate" ]; then
      BIN=$(readlink -f "$candidate")
      echo "note: no 'darkroom' on PATH; pointing the entry at $BIN"
      echo "      (a symlink into ~/.local/bin survives 'cargo clean'; this does not)"
      break
    fi
  done
fi
if [ -z "$BIN" ]; then
  echo "warning: no darkroom binary found — on PATH, in target/release, or in" >&2
  echo "         target/debug, and DARKROOM_BIN is unset. The entry will be" >&2
  echo "         installed but ignored until one exists. Build first, or set" >&2
  echo "         DARKROOM_BIN=/path/to/darkroom." >&2
  BIN=darkroom
fi

echo "installing hicolor PNGs into $DATA/icons/hicolor"
for n in 16 24 32 48 64 128 256 512; do
  install -Dm644 "$ICONS/darkroom-$n.png" \
    "$DATA/icons/hicolor/${n}x${n}/apps/darkroom.png"
  # The same artwork under the type's default icon name, so file managers
  # give `.darkroom` documents an icon of their own rather than the generic
  # archive one.
  install -Dm644 "$ICONS/darkroom-$n.png" \
    "$DATA/icons/hicolor/${n}x${n}/mimetypes/application-x-darkroom.png"
done

echo "installing MIME type into $DATA/mime/packages"
install -Dm644 darkroom-mime.xml "$DATA/mime/packages/darkroom.xml"

echo "installing desktop entry into $DATA/applications (Exec=$BIN)"
entry=$(mktemp)
trap 'rm -f "$entry"' EXIT
sed "s|^Exec=darkroom |Exec=$BIN |" darkroom.desktop >"$entry"
install -Dm644 "$entry" "$DATA/applications/darkroom.desktop"

# Refresh caches (best-effort; harmless if the tools are absent).
gtk-update-icon-cache -f -t "$DATA/icons/hicolor" 2>/dev/null || true
update-mime-database "$DATA/mime" 2>/dev/null || true
update-desktop-database "$DATA/applications" 2>/dev/null || true

# Claim the type as the default handler. Per-user only: this writes the
# caller's mimeapps.list, which a PREFIX (system-wide) install has no business
# touching — there the admin's `xdg-mime default` or the user's own file
# decides.
if [ -z "${PREFIX:-}" ]; then
  xdg-mime default darkroom.desktop application/x-darkroom 2>/dev/null || true
fi

echo "done. darkroom should now appear in your application launcher."

# Check the outcome rather than trusting the steps. `xdg-mime query default`
# only reads back the line just written to mimeapps.list, so it answers
# correctly even when the entry is unusable; `gio mime` resolves it the way a
# file manager does, and is what catches an unloadable entry.
if command -v gio >/dev/null 2>&1; then
  if gio mime application/x-darkroom 2>/dev/null | grep -q 'darkroom\.desktop'; then
    echo "verified: .darkroom files resolve to darkroom.desktop"
  else
    echo >&2
    echo "warning: the type is installed but no application resolves for it." >&2
    echo "         The usual cause is an Exec binary that cannot be found, which" >&2
    echo "         makes the whole entry invalid. Check:" >&2
    echo "           gio mime application/x-darkroom" >&2
    echo "           command -v $BIN" >&2
  fi
fi
