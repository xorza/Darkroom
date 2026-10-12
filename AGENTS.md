# Imaginarium

An image-processing library with **CPU (SIMD: SSE/AVX2/NEON) and GPU (wgpu
compute) backends**, one CPU/GPU image buffer, and 9 pixel formats. A
standalone crate, used in Darkroom by `lens`, `lumos` and `darkroom`.
Pre-alpha: rename, re-sign and rewrite callers freely; no compat shims.

- **GPU is optional**, behind the `wgpu` feature (off by default). CPU paths
  compile and pass without it.
- **Every SIMD kernel has a scalar reference** and a test comparing the two.
- **All 9 formats.** A new op or shader handles the full format set, or
  declares the narrower list it supports.
- **One rounding rule.** Narrowing into an integer sample rounds to nearest,
  ties to even, and saturates (`common::sample::Sample`); SIMD kernels and
  WGSL (`gpu/packed.wgsl`) follow it, and every GPU test compares with the
  CPU result.

## Verification

```
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --tests --all-features
cargo clippy --all-targets -- -D warnings
cargo clippy -- -D warnings
```

The fourth line is the CPU-only leg; the first gate on `not(feature = "wgpu")`
adds `cargo test --tests`. The last lints the build that ships: every other
target turns `internals` on through the self dev-dependency. A change to bench
code adds `cargo benches -- --test`, which runs each case once.
`.cargo/config.toml` builds x86-64 for x86-64-v3, the CPU darkroom builds for.

`neon.rs` files and the aarch64 arms of the dispatch tables are aarch64-only,
so the x86 chain never compiles them. A change to one adds
`cargo clippy --target aarch64-unknown-linux-gnu --all-targets --all-features -- -D warnings`
(after `rustup target add aarch64-unknown-linux-gnu`; clippy does not link).
Running the NEON tests needs `qemu-user` and a runner in
`.cargo/config.toml`; without them, say the kernel was only compile-checked.

## Test code

`internals` is the one feature for what only tests and benches build; the
crate's dev-dependency on itself turns it on in every test and bench build.
Code a bench reaches is gated `#[cfg(feature = "internals")]`; a reach-in only
the crate's own tests call is gated `#[cfg(test)]`. Never
`any(test, feature = "internals")`.

## Benches

One target runs every case through `bench::run`; each driver sits in a
`bench.rs` beside the code it measures, and `src/bench/mod.rs` states the
rules. `cargo benches` runs them with fixed features (`wgpu`), so a case
always measures the same build. An id is `<kind>/<group>/<case>`, the case the
variant (a tier, a backend) then the input: an `atomic` case times one kernel
on the unit dispatch hands it, for the scalar reference and each tier the CPU
has; an `integration` case times a public entry point on the 24 MP standard
frame, on each backend. A filter selects by id: `cargo benches -- --list`,
`-- '^atomic/convert/'`, `-- --exact integration/preview/cpu/rgb_f32`.
