#!/bin/sh
set -eu

# Build targets:
# - Linux x86_64 gnu (native)
# - Windows x86_64 (MinGW) - requires: pacman -S mingw-w64-gcc
# - Linux aarch64 gnu (ARMv8) - requires: pacman -S aarch64-linux-gnu-gcc
#
# Cross-compilation targets not included:
# - aarch64-apple-darwin - requires osxcross
#
# Usage:
#   ./build-all.sh       - Build for all targets
#   ./build-all.sh -i    - Install all rustup targets first, then build

TARGETS="
x86_64-unknown-linux-gnu
x86_64-pc-windows-gnu
aarch64-unknown-linux-gnu
"

# Handle -i flag to install targets
if [ "${1:-}" = "-i" ]; then
  echo "==> Installing rustup targets"
  for t in $TARGETS; do
    echo "==> rustup target add $t"
    rustup target add "$t"
  done
fi

echo "==> Running clippy for all targets"
for t in $TARGETS; do
  echo "==> cargo clippy --target $t"
  cargo clippy --target "$t" --all-targets -- -D warnings
done

echo "==> Building for all targets"
for t in $TARGETS; do
  echo "==> cargo build --target $t"
  cargo build --target "$t"
done

echo "==> Done."
