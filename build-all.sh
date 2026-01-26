#!/bin/sh
set -eu

# Build targets:
# - Linux x86_64 musl
# - Linux aarch64 musl (ARMv8)
# - macOS aarch64 (Apple Silicon)
#
# Notes:
# - rustup targets must be installed beforehand:
#   rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl aarch64-apple-darwin
# - Cross-compiling musl targets may require a linker/toolchain (or using zig).

TARGETS="
x86_64-unknown-linux-gnu
x86_64-unknown-linux-musl
x86_64-pc-windows-gnu
aarch64-unknown-linux-gnu
aarch64-unknown-linux-musl
aarch64-apple-darwin
"

echo "==> Running clippy (host target)"
cargo clippy --all-targets -- -D warnings

echo "==> Building release for all targets"
for t in $TARGETS; do
  echo "==> cargo build --release --target $t"
  cargo build --release --target "$t"
done

echo "==> Done."
