#!/bin/sh
# Counts the calls to a vector op that fell out of line in lumos's x86 build: an AVX2 helper or a
# `core_arch` intrinsic that a `#[target_feature]` entry (a `5enter` symbol) did not inline runs
# without AVX2 and FMA below x86-64-v3, and no test fails. Prints the count; 0 passes. The build
# is the host's own: LibRaw's C++ does not cross-compile.
set -eu
target_dir=$1
if [ "$(uname -m)" != x86_64 ]; then
    echo "the inline check needs an x86_64 host" >&2
    exit 1
fi
cargo rustc --locked -p lumos --release --lib -- --emit=asm
asm=$(ls -t "$target_dir/release/deps"/lumos-*.s | head -1)
count=$(grep -E "call.*(Avx2|core_arch)" "$asm" | grep -vc 5enter || true)
echo "$count"
[ "$count" -eq 0 ]
