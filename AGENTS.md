# Imaginarium

An image-processing library with **CPU (SIMD: SSE/AVX2/NEON) and GPU (wgpu
compute) backends**, a unified CPU/GPU image buffer, and 9-format pixel
polymorphism. A standalone crate, used in Darkroom by `lens`, `lumos`, and
`darkroom`. Pre-alpha: rename, re-sign, and rewrite callers freely; no compat
shims.

- **GPU is optional**, behind the `wgpu` feature (`default = []`). CPU paths
  must compile and pass with it off.
- **Every SIMD kernel has a scalar reference** and a test comparing the two.
  No SIMD path lands without both.
- **9 formats, always.** New ops and shaders handle the full format set, or
  declare a narrower supported list. Half-covering the matrix is a bug.

## Verification

```
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --tests --all-features
cargo clippy --all-targets --features bench -- -D warnings
```

The last line is the CPU-only leg (every feature but `wgpu`). It needs no test
run while nothing is gated `not(feature = "wgpu")`; the first such gate adds
`cargo test --tests --features bench`.

`neon.rs` files and dispatch tables are aarch64-only, so the x86 chain never
compiles them. When one changes, add:

```
cargo clippy -p imaginarium --target aarch64-unknown-linux-gnu --all-targets --all-features -- -D warnings
```

It needs only `rustup target add aarch64-unknown-linux-gnu` — clippy does not
link. Running the NEON tests needs `qemu-user` and a runner in
`.cargo/config.toml`; without them, say the kernel was only compile-checked.

## Benchmarks

Criterion drivers live in `bench.rs` beside the code they measure, gated
`#[cfg(feature = "bench")]` and exposed only through the `crate::bench`
facade. Every bench target requires the feature:
`cargo bench --features bench --bench contrast_brightness`.
