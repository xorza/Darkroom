#!/usr/bin/env bash
# Install darkroom's icon + desktop entry into the per-user XDG data dirs so
# GNOME/KDE/etc. show the icon in launchers and the taskbar. Run after
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

echo "installing hicolor PNGs into $DATA/icons/hicolor"
for n in 16 24 32 48 64 128 256 512; do
  install -Dm644 "$ICONS/darkroom-$n.png" \
    "$DATA/icons/hicolor/${n}x${n}/apps/darkroom.png"
done

echo "installing desktop entry into $DATA/applications"
install -Dm644 darkroom.desktop "$DATA/applications/darkroom.desktop"

# Refresh caches (best-effort; harmless if the tools are absent).
gtk-update-icon-cache -f -t "$DATA/icons/hicolor" 2>/dev/null || true
update-desktop-database "$DATA/applications" 2>/dev/null || true

echo "done. darkroom should now appear in your application launcher."
