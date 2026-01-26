#!/bin/sh
set -eu

# Build targets:
# - Linux x86_64 gnu (native)
# - Windows x86_64 (MinGW) - requires: pacman -S mingw-w64-gcc
#
# Cross-compilation targets (require additional toolchains, not enabled by default):
# - aarch64-unknown-linux-gnu - requires: aarch64-linux-gnu-gcc
# - aarch64-apple-darwin - requires: osxcross
#
# Usage:
#   ./build-all.sh       - Build for all targets
#   ./build-all.sh -i    - Install all rustup targets first, then build

TARGETS="
x86_64-unknown-linux-gnu
x86_64-pc-windows-gnu
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

echo "==> Building release for all targets"
for t in $TARGETS; do
  echo "==> cargo build --release --target $t"
  cargo build --release --target "$t"
done

echo "==> Done."
