#!/usr/bin/env bash
# Regenerate every platform icon artifact from the master SVG.
# Requires: rsvg-convert (librsvg), magick (ImageMagick), python3.
#
#   darkroom/assets/icons/generate.sh
#
# Outputs (this directory):
#   darkroom-<N>.png     PNG set: 16 24 32 48 64 128 256 512 1024
#   darkroom.ico         Windows multi-res icon (16..256) — for build.rs / winres
#   darkroom.icns        macOS icon (PNG-in-ICNS, 32..1024) — for the .app bundle
#   darkroom.iconset/    macOS iconutil source (fallback: `iconutil -c icns`)
set -euo pipefail
cd "$(dirname "$0")"
SRC="../logo.svg"
SIZES="16 24 32 48 64 128 256 512 1024"

echo "rendering PNGs from $SRC"
for n in $SIZES; do
  rsvg-convert -w "$n" -h "$n" "$SRC" -o "darkroom-$n.png"
done

echo "building darkroom.ico"
magick darkroom-256.png darkroom-64.png darkroom-48.png darkroom-32.png darkroom-16.png darkroom.ico

echo "building darkroom.iconset (for macOS iconutil)"
rm -rf darkroom.iconset && mkdir darkroom.iconset
cp darkroom-16.png   darkroom.iconset/icon_16x16.png
cp darkroom-32.png   darkroom.iconset/icon_16x16@2x.png
cp darkroom-32.png   darkroom.iconset/icon_32x32.png
cp darkroom-64.png   darkroom.iconset/icon_32x32@2x.png
cp darkroom-128.png  darkroom.iconset/icon_128x128.png
cp darkroom-256.png  darkroom.iconset/icon_128x128@2x.png
cp darkroom-256.png  darkroom.iconset/icon_256x256.png
cp darkroom-512.png  darkroom.iconset/icon_256x256@2x.png
cp darkroom-512.png  darkroom.iconset/icon_512x512.png
cp darkroom-1024.png darkroom.iconset/icon_512x512@2x.png

echo "building darkroom.icns (PNG-in-ICNS, no macOS tools needed)"
python3 - <<'PY'
import struct
# OSType -> source PNG size. These codes take a raw PNG payload on macOS 10.7+.
entries = [
    (b"ic11", 32),   # 16pt @2x
    (b"ic12", 64),   # 32pt @2x
    (b"ic07", 128),  # 128pt @1x
    (b"ic08", 256),  # 256pt @1x
    (b"ic13", 256),  # 128pt @2x
    (b"ic09", 512),  # 512pt @1x
    (b"ic14", 512),  # 256pt @2x
    (b"ic10", 1024), # 512pt @2x
]
blobs = b""
for ostype, size in entries:
    with open(f"darkroom-{size}.png", "rb") as f:
        png = f.read()
    blobs += ostype + struct.pack(">I", len(png) + 8) + png
data = b"icns" + struct.pack(">I", len(blobs) + 8) + blobs
with open("darkroom.icns", "wb") as f:
    f.write(data)
print(f"  darkroom.icns: {len(data)} bytes, {len(entries)} sizes")
PY

echo "done."
ls -1
