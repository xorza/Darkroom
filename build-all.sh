#!/bin/sh
set -eu

# Build targets:
# - Linux x86_64 gnu
# - Linux aarch64 gnu (ARMv8)
# - Windows x86_64 (MinGW)
# - macOS aarch64 (Apple Silicon)
#
# Usage:
#   ./build-all.sh       - Build for all targets
#   ./build-all.sh -i    - Install all rustup targets first, then build

TARGETS="
x86_64-unknown-linux-gnu
x86_64-pc-windows-gnu
aarch64-unknown-linux-gnu
aarch64-apple-darwin
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
