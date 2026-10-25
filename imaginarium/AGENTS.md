# Imaginarium

Interleaved images in nine pixel formats, with **CPU (SIMD: SSE/AVX2/NEON)
and GPU (wgpu compute) backends**: the codecs (PNG, JPEG, TIFF) and the ops
(blend, contrast and brightness, affine resample, preview). lens runs its ops
as nodes; lumos reads rasters through its codecs.

- **GPU is optional**, behind the `wgpu` feature (off by default). CPU paths
  compile and pass without it.
- **Every SIMD kernel has a scalar reference** and a test comparing the two.
- **All 9 formats.** A new op or shader handles the full format set, or
  declares the narrower list it supports.
- **One rounding rule.** Narrowing into an integer sample rounds to nearest,
  ties to even, and saturates (`color::sample::Sample`); SIMD kernels and
  WGSL (`gpu/packed.wgsl`) follow it, and every GPU test compares with the
  CPU result.

## Verification

`cargo drive check`, from the darkroom root (its Verification section says
what it runs). The root lints this crate twice, with `wgpu` and without it,
and its benches always measure the GPU build.

The SSE and AVX kernels are x86-only, so an arm64 host never compiles them. A
change to one there adds
`cargo clippy -p imaginarium --target x86_64-apple-darwin --all-targets --all-features -- -D warnings`.
The root's `cross-lint` cannot do it: that lane lints every feature, and
`ort-sys` has no `x86_64-apple-darwin` binaries for lumos's `ml`.

`neon.rs` files and the aarch64 arms of the dispatch tables are aarch64-only,
so an x86 host never compiles them. A change to one there adds
`cargo clippy -p imaginarium --target aarch64-unknown-linux-gnu --all-targets --all-features -- -D warnings`
(after `rustup target add aarch64-unknown-linux-gnu`; clippy does not link).
Running the NEON tests needs `qemu-user` and a runner in the root
`.cargo/config.toml`; without them, say the kernel was only compile-checked.

## Benches

Each driver sits in a `bench.rs` beside the code it measures, and
`src/bench/mod.rs` states the rules. An id is
`<kind>/<group>/<backend>/<rest…>`: an `atomic` case times one kernel on the
unit dispatch hands it, for the scalar reference and each tier the CPU has; an
`integration` case times a public entry point on the 24 MP standard frame, on
each backend. A filter selects by id: `cargo drive bench --list`,
`'^atomic/convert/'`, `--exact integration/preview/cpu/rgb_f32`.
