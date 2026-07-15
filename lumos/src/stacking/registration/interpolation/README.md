# Registration Interpolation

This module owns sampling kernels and optimized row execution. Public image-level orchestration
lives in `../resample.rs`; it calls `warp_image` for each channel and `warp_coverage` once per frame.

## Layout

| Path | Responsibility |
|------|----------------|
| `mod.rs` | `WarpParams`, scalar kernels, coverage evaluation, method dispatch |
| `warp/mod.rs` | Incremental row traversal, scalar bilinear/Lanczos, SIMD dispatch |
| `warp/sse.rs` | x86 AVX2/SSE4.1 bilinear and 128-bit FMA Lanczos kernels |
| `warp/neon.rs` | aarch64 NEON bilinear and Lanczos kernels |
| `tests.rs` | Kernel, coverage, and full-warp correctness |
| `bench.rs` | Ignored quickbench performance tests |

## Methods

`WarpParams` combines an `InterpolationMethod` with a constant out-of-bounds value. Supported
methods are nearest, bilinear, Catmull–Rom bicubic, and Lanczos-2/3/4. Lanczos variants carry a
soft-clamp deringing threshold; a negative value disables deringing.

`warp_image` selects the row path by method:

- nearest and bicubic evaluate the scalar sampler along an incremental output row;
- bilinear uses AVX2/SSE4.1 or NEON when the transform and border permit it, then scalar tails;
- every Lanczos radius uses the const-generic interior kernel, with x86 FMA or NEON acceleration
  where available and scalar border handling.

All paths preserve the same inverse-mapping convention and are cross-checked against scalar
references.

## Coverage and borders

`warp_coverage` evaluates the same inverse-mapped coordinates and kernel support as the image
warp. `resample::warp` emits that map beside the aligned image. For nearest and bilinear with a zero
border, partial pixels are renormalized by coverage; negative-lobe bicubic/Lanczos output is not
renormalized.

Lanczos weights come from `OnceLock` LUTs at 4096 samples per unit. The Lanczos-3 table is 48 KiB.
Affine/projective rows use incremental `f64` coordinates, and fully in-bounds kernels skip per-tap
bounds checks.
