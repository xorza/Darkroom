# Lumos

Astronomical stacking pipeline: **load / decode (RAW, FITS) → calibrate →
detect stars → register → combine (stack or drizzle)**, then an optional
non-linear **stretch**, strictly after all linear-domain work. CPU only:
vector kernels written once over `simd::Isa` (AVX2+FMA, NEON, a portable
fallback; see `src/simd/mod.rs`) and rayon. Pixels are **planar** f32, one
plane per channel, normalized to `[0, 1]`.

## Scope

The **most precise and the fastest** stacking pipeline, growing toward a
**science data product**: the linear master plus per-pixel quality planes
(coverage, weight, variance) a downstream tool measures it with — photometry,
source extraction, error bars.

- **The stacked master comes first.** A science extra must stay simple and
  ride on data the pipeline already computes. Machinery that serves neither
  the image nor its measurement is removed, not carried.
- **Thin-plate-spline distortion (`registration/distortion/tps/`) and drizzle
  (`drizzle/`) stay**, even without a production caller.
- **Precision outranks speed.** Never trade the stacked result's accuracy for
  throughput.

## Verification

`cargo drive check`, from the darkroom root (its Verification section says
what it runs). Tests leave out `real-data`, as darkroom's
`[workspace.metadata.drive]` says.

`real-data` runs the tests that read the gitignored ~7.4 GB dataset in
`test_data/lumos_data/`, only when asked. Its ML tests skip without ONNX
weights (`STARNET2_ONNX` / `DEEPSNR_ONNX`, or the defaults in `test_data/`).

A change to a vector kernel (anything under `simd::Isa`) also checks that
nothing fell out of line: a function the `#[target_feature]` entry does not
inline runs without AVX2 and FMA below x86-64-v3, an order of magnitude
slower, and no test fails. This must print `0`:

```
cargo rustc -p lumos --release --lib -- --emit=asm && grep -E "call.*(Avx2|core_arch)" $(ls -t ../target/release/deps/lumos-*.s | head -1) | grep -vc 5enter
```

It catches a missing `#[inline(always)]` and a closure that calls a vector
op. Plain AVX intrinsics prove nothing: the workspace builds for x86-64-v3
(`.cargo/config.toml`). That flag may be turned off, so keep the runtime ISA
dispatch, and every kernel correct and fast without it.

## Benches

Criterion benches live in `bench.rs` beside the code they measure and run
from the one target `benches/lumos.rs` through `bench::run`:
`cargo drive bench <filter>`, `--test` to run each once.
`src/bench/mod.rs` states their shape.
