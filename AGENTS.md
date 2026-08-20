# Imaginarium

An image-processing library with **CPU (SIMD: SSE/AVX2/NEON) and GPU (wgpu
compute) backends**, a unified CPU/GPU image buffer, and 9-format pixel
polymorphism. A workspace member of Scenarium and a git submodule; consumed by
`lens`/`lumos`. Pre-alpha, breaks freely.

## Posture

- **No stability guarantees.** Rename, refactor, change signatures and rewrite
  callers. No compat shims.
- **GPU is optional, gated behind the `wgpu` feature** (`default = []`). CPU
  paths must compile and pass with the feature off.
- **SIMD correctness is cross-checked against scalar.** Every SIMD kernel has a
  scalar reference; tests compare the two. Don't add a SIMD path without the
  scalar fallback and the cross-check.
- **9 formats, always.** New ops and shaders handle the full format set (or
  explicitly declare a narrower supported list). Half-covering the format
  matrix is a bug.

## Benchmarks

Criterion. `bench.rs` beside the code it measures, gated
`#[cfg(feature = "bench")]` and reached only through the `crate::bench` facade,
so `benches/*.rs` stays harness wiring and no production module goes `pub` for a
benchmark's sake. `bench` implies `internals`.

```
cargo bench -p imaginarium --bench contrast_brightness
```

Name the target. An unfiltered `cargo bench` links every bench binary at once
under a fat-LTO profile and can get OOM-killed; cap with `-j 2` if you need
several.

## Cross-arch verification

Half the SIMD here is `aarch64`-only, so an x86 host's usual chain never
compiles it and a NEON kernel can rot silently. Whenever a `neon.rs` or a
dispatch table changes, add the cross-target leg:

```
cargo clippy -p imaginarium --target aarch64-unknown-linux-gnu --all-targets --all-features -- -D warnings
```

`clippy` and `check` need only `rustup target add aarch64-unknown-linux-gnu` —
neither links, so no cross linker is required. **Running** the NEON tests needs
`qemu-user` plus a runner in `.cargo/config.toml`; without it a NEON kernel is
compile-checked on an x86 host and proven only by the scalar cross-check tests
on real hardware. Say which of the two you did.
