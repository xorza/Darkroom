#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ICON_DIR="$HOME/.local/share/icons/hicolor"
APP_DIR="$HOME/.local/share/applications"
SOURCE_ICON="$SCRIPT_DIR/prism.png"

echo "Installing Prism icon and desktop entry..."

# Generate and install icons at standard sizes
for size in 16 32 48 64 128; do
    mkdir -p "$ICON_DIR/${size}x${size}/apps"
    magick "$SOURCE_ICON" -resize "${size}x${size}" "$ICON_DIR/${size}x${size}/apps/prism.png"
    echo "Installed ${size}x${size} icon"
done

# Install desktop file
mkdir -p "$APP_DIR"
cp "$SCRIPT_DIR/prism.desktop" "$APP_DIR/prism.desktop"
echo "Installed desktop entry to $APP_DIR/prism.desktop"

# Update icon cache
if command -v gtk-update-icon-cache &> /dev/null; then
    gtk-update-icon-cache -f -t "$ICON_DIR" 2>/dev/null || true
    echo "Updated icon cache"
fi

# Update desktop database
if command -v update-desktop-database &> /dev/null; then
    update-desktop-database "$APP_DIR" 2>/dev/null || true
    echo "Updated desktop database"
fi

echo "Done! You may need to log out and back in for changes to take effect."
