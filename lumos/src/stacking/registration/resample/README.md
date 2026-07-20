# Registration Resampling

This module owns the complete resampling stack. `mod.rs` orchestrates image channels and quality
maps, while the child modules separate plane dispatch, scalar kernels, row execution, and quality
evaluation.

## Layout

| Path | Responsibility |
|------|----------------|
| `mod.rs` | Image-level `warp` orchestration and `WarpResult` |
| `plane/` | Single-plane method dispatch |
| `kernel/` | Scalar kernels, Lanczos LUTs, and point-sampling references |
| `quality/` | Coverage and interpolation-confidence maps |
| `row/` | Incremental row traversal, scalar/SIMD bilinear, and Lanczos execution |
| `row/sse/` | x86 AVX2/SSE4.1 bilinear and FMA Lanczos kernels |
| `row/neon.rs` | aarch64 NEON bilinear and Lanczos kernels |
| `tests.rs` | Image-level border and quality integration tests |
| `bench.rs` | Ignored quickbench performance tests |

## Methods

`config.rs` defines `WarpParams`, which combines an `InterpolationMethod` with a constant
out-of-bounds value. Supported methods are nearest, bilinear, Catmull–Rom bicubic, and
Lanczos-2/3/4.

`plane::warp` selects the row path by method:

- nearest and bicubic evaluate the scalar sampler along an incremental output row;
- bilinear uses AVX2/SSE4.1 or NEON when the transform and border permit it, then scalar tails;
- every Lanczos radius uses the const-generic interior kernel, with x86 FMA or NEON acceleration
  where available. Fully supported kernels use normalized linear Lanczos; partial kernels use
  edge-extended bilinear interpolation so signed data cannot be amplified by a near-zero truncated
  kernel sum.

All paths preserve the same inverse-mapping convention and are cross-checked against scalar
references.

## Quality maps and borders

`quality::maps` evaluates the same inverse-mapped coordinates and kernel support as the plane warp.
Coverage uses the in-bounds fraction of absolute kernel weight, so negative lobes cannot cancel
geometric support. Confidence is the inverse white-noise variance derived from normalized
coefficient energy. Stacking uses coverage only as an inclusion gate and confidence only as a
weight multiplier.

Source positions outside the closed pixel footprint
`[-0.5, width - 0.5] × [-0.5, height - 0.5]` receive the configured border value and zero
coverage/confidence. Positions inside that footprint use only real source pixels: bilinear clamps
to the nearest edge center for partial support, bicubic drops unavailable taps and normalizes the
signed sum, and partial Lanczos kernels use stable edge-extended bilinear. Lanczos confidence
follows that fallback while coverage continues to describe the nominal Lanczos support.

Lanczos weights come from `OnceLock` LUTs at 4096 samples per unit. The Lanczos-3 table is 48 KiB.
Linear rows use incremental `f64` coordinates; projective and SIP rows evaluate each output
position independently. Fully in-bounds kernels skip per-tap bounds checks.
