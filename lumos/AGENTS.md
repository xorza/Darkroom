# Lumos

Astronomical stacking pipeline: **load / decode (RAW, FITS) → calibrate →
detect stars → register → combine (stack or drizzle)**, then an optional
non-linear **stretch** into the display domain, strictly after all
linear-domain work. CPU only — vector kernels written once over
`simd::Isa` (AVX2+FMA, NEON, and a portable fallback; see `src/simd/mod.rs`)
and rayon. Pixels are **planar** (one f32 plane per channel), normalized to
`[0, 1]`.

## Scope

The **most precise and the fastest** stacking pipeline, growing toward a
**science data product**: the linear stacked master _plus_ per-pixel quality
planes (coverage, weight, variance/noise) that let a downstream tool
**measure** it — photometry, source extraction, error bars.

- **The stacked master comes first.** Science extras are welcome only when
  they stay low-complexity and ride on data the pipeline already computes.
  Machinery that serves neither the image nor its measurability is out of
  scope — remove it rather than carry it.
- **Thin-plate-spline distortion (`registration/distortion/tps/`) and drizzle
  (`drizzle/`) stay.** Both are in scope even without a production caller —
  do not propose removing them.
- **Precision and correctness outrank speed.** When they conflict, the
  numerically-correct choice wins; never trade accuracy of the stacked result
  for throughput.

## Verification

Tests leave out `real-data`:

```
cargo test -p lumos --tests --features ml
```

Benches are quickbench `#[test] #[ignore]` functions in `bench.rs` files, compiled only with
the `bench` feature: `cargo test -p lumos --release --features bench <filter> -- --ignored --nocapture`.

A change to a vector kernel (anything under `simd::Isa`) is also checked for code that fell out
of line. A function the `#[target_feature]` entry does not inline runs without AVX2 and FMA, so
the kernel stays correct but slows by an order of magnitude, and no test fails. This must print
`0`:

```
cargo rustc -p lumos --release --lib -- --emit=asm && grep -E "call.*(Avx2|core_arch)" $(ls -t ../target/release/deps/lumos-*.s | head -1) | grep -vc 5enter
```

It catches a missing `#[inline(always)]` and a closure that calls a vector op. Plain AVX
intrinsics are no witness: the workspace enables `+f16c`, which implies AVX everywhere.

`real-data` runs the tests that read the gitignored ~7.4 GB dataset in
`test_data/lumos_data/` — only when asked. Its ML tests also need ONNX weights
(`STARNET2_ONNX` / `DEEPSNR_ONNX`, or the default files in `test_data/`) and
skip without them.
