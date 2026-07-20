# Registration Interpolation

This module owns sampling kernels and optimized row execution. Public image-level orchestration
lives in `../resample.rs`; it calls `warp_image` for each channel and `warp_quality_maps` once per
frame.

## Layout

| Path | Responsibility |
|------|----------------|
| `mod.rs` | `WarpParams`, scalar kernels, quality-map evaluation, method dispatch |
| `warp/mod.rs` | Incremental row traversal, scalar bilinear/Lanczos, SIMD dispatch |
| `warp/sse.rs` | x86 AVX2/SSE4.1 bilinear and 128-bit FMA Lanczos kernels |
| `warp/neon.rs` | aarch64 NEON bilinear and Lanczos kernels |
| `tests.rs` | Kernel, coverage, and full-warp correctness |
| `bench.rs` | Ignored quickbench performance tests |

## Methods

`WarpParams` combines an `InterpolationMethod` with a constant out-of-bounds value. Supported
methods are nearest, bilinear, Catmull–Rom bicubic, and Lanczos-2/3/4.

`warp_image` selects the row path by method:

- nearest and bicubic evaluate the scalar sampler along an incremental output row;
- bilinear uses AVX2/SSE4.1 or NEON when the transform and border permit it, then scalar tails;
- every Lanczos radius uses the const-generic interior kernel, with x86 FMA or NEON acceleration
  where available. Fully supported kernels use normalized linear Lanczos; partial kernels use
  edge-extended bilinear interpolation so signed data cannot be amplified by a near-zero truncated
  kernel sum.

All paths preserve the same inverse-mapping convention and are cross-checked against scalar
references.

## Quality maps and borders

`warp_quality_maps` evaluates the same inverse-mapped coordinates and kernel support as the image
warp. Coverage uses the in-bounds fraction of absolute kernel weight, so negative lobes cannot
cancel geometric support. Confidence is the inverse white-noise variance derived from normalized
coefficient energy. Stacking uses coverage only as an inclusion gate and confidence only as a
weight multiplier.

Zero-border bilinear samples use a separate signed-weight sum for partial-pixel normalization.
Partial Lanczos kernels use edge-extended bilinear, and their confidence follows that fallback
while coverage continues to describe the nominal Lanczos support.

Lanczos weights come from `OnceLock` LUTs at 4096 samples per unit. The Lanczos-3 table is 48 KiB.
Affine/projective rows use incremental `f64` coordinates, and fully in-bounds kernels skip per-tap
bounds checks.
