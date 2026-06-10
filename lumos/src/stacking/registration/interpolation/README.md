# Interpolation Module

Sub-pixel image resampling for registration warping. Supports nearest, bilinear, bicubic, and Lanczos (2/3/4) interpolation methods.

## Architecture

- `mod.rs` — Kernel functions (`interpolate_*`), LUT infrastructure, `warp_image` entry point
- `warp/` — Optimized row-warping dispatch (called from `warp_image` for bilinear and Lanczos3)
  - `mod.rs` — Scalar implementations with incremental stepping and bounds-check fast paths
  - `sse.rs` — x86_64 AVX2/SSE4.1 bilinear (8/4 pixels per cycle)
- `tests.rs` — Interpolation quality and warp correctness tests
- `bench.rs` — Performance benchmarks for optimization tracking

## Optimized Paths

`warp_image` dispatches to optimized row-warping for two methods:

- **Bilinear**: AVX2/SSE4.1 on x86_64, scalar with incremental stepping on aarch64
- **Lanczos3**: Scalar with incremental stepping + fast-path interior bounds skipping (all platforms)

Other methods (nearest, bicubic, Lanczos2, Lanczos4) use per-pixel `interpolate()`.

## SIMD Optimization Decisions

### Bilinear — x86_64 AVX2/SSE4.1: kept

AVX2 processes 8 output pixels per cycle, SSE4.1 processes 4. Scalar fallback with incremental coordinate stepping used on aarch64. These SIMD paths provide measurable speedup on x86_64 because bilinear only samples 4 pixels (2x2) — the small kernel keeps the operation compute-bound rather than memory-bound.

### Bilinear — aarch64 NEON: removed

Benchmarked and found slower than scalar. The NEON gather overhead (no hardware gather instruction, must do scalar loads + `vld1q_lane_f32`) negated any arithmetic savings for a 2x2 kernel.

### Lanczos3 — aarch64 NEON: tested, not adopted

Two approaches benchmarked, neither showed improvement over scalar:

1. **Per-row horizontal reduction**: `vld1q_f32` + `vmulq_f32` for 4 of 6 taps, `vaddvq_f32` horizontal sum, plus 2 scalar taps. The horizontal reduction per kernel row negated gains.

2. **Vertical accumulation**: `vfmaq_f32` accumulating wy-weighted pixels across all 6 rows into a single `float32x4_t`, single horizontal reduction at the end. Still no improvement.

Root cause: Lanczos3 is memory-bound. The 6x6 kernel accesses 6 non-contiguous rows at arbitrary positions determined by the geometric transform. Cache misses dominate, not arithmetic. The compiler's auto-vectorization of the scalar fast-path (which uses `get_unchecked` for the inner 6 multiplies) already generates efficient code.

### Lanczos3 — x86_64 AVX2: not attempted

Same memory-bound analysis applies. AVX2 gather instructions (`_mm256_i32gather_ps`) have high latency and the random access pattern across kernel rows would not benefit.

## Key Algorithmic Optimizations (non-SIMD)

These scalar optimizations in `warp_row_lanczos3` achieved a **38% speedup** (602ms to 371ms on real data):

1. **Incremental coordinate stepping**: For affine transforms, source coordinates step by constant `(m[0], m[3])` per output pixel, avoiding per-pixel matrix multiply.
2. **Bounds-check fast path**: One bounds check for the entire 6x6 kernel. Interior pixels use `get_unchecked` direct indexing; border pixels fall back to per-tap bounds checking.
3. **Row pointer caching**: `row_offset = (ky + j) * width + kx` computed once per kernel row.

## Lanczos LUT

Lanczos kernel weights use a precomputed LUT (4096 samples per unit) stored in `OnceLock` statics. The Lanczos3 LUT is 48KB (fits in L1 cache). LUT lookup uses rounded index with `get_unchecked` for zero-overhead access.
