# raw/ - RAW Camera File Loading and Demosaicing

## Architecture

```
raw/
├── mod.rs              # Entry point: load_raw(), RAII guards, sensor dispatch
├── normalize.rs        # SIMD u16-to-f32 normalization (SSE2/SSE4.1/NEON)
├── tests.rs            # Unit tests for load_raw and normalization
├── benches.rs          # Benchmarks and quality comparison vs libraw
└── demosaic/
    ├── mod.rs           # Re-exports bayer and xtrans
    ├── bayer/
    │   ├── mod.rs       # Dispatcher: parallel/SIMD/scalar paths
    │   ├── scalar.rs    # Scalar bilinear interpolation functions
    │   ├── simd_sse3.rs # SSE3 full-image demosaic (non-parallel path)
    │   ├── simd_neon.rs # NEON full-image demosaic (non-parallel path)
    │   └── tests.rs     # 25 tests covering all CFA patterns and code paths
    └── xtrans/
        ├── mod.rs              # XTransPattern, XTransImage, process_xtrans()
        ├── hex_lookup.rs       # Hexagonal neighbor tables for Markesteijn
        ├── markesteijn.rs      # 1-pass orchestrator and buffer management
        └── markesteijn_steps.rs # 6-step algorithm: green, RGB, derivatives, blend
```

## Pipeline Flow

`AstroImage::from_file()` dispatches RAW extensions to `raw::load_raw(path)`, which:

1. Reads file into memory, opens via libraw FFI (`libraw_open_buffer`, `libraw_unpack`)
2. Extracts dimensions, margins, black/maximum levels
3. Calls `detect_sensor_type()` on libraw's `filters`/`colors` fields
4. Dispatches to one of four paths:

| Sensor Type | Normalization | Demosaic | Output |
|------------|---------------|----------|--------|
| Monochrome | Inline parallel (rayon) | None | 1-channel grayscale |
| Bayer | `normalize_u16_to_f32_parallel()` (SIMD) | `demosaic_bilinear()` (SIMD+rayon) | 3-channel RGB |
| X-Trans | Scalar single-threaded | Markesteijn 1-pass (rayon) | 3-channel RGB |
| Unknown | libraw `dcraw_process` | libraw built-in | 3-channel RGB |

## SIMD Strategy

### Normalization (`normalize.rs`)
- **SSE4.1** (preferred): `_mm_cvtepu16_epi32` for u16 zero-extension
- **SSE2** (fallback): `_mm_unpacklo_epi16` with zero register
- **NEON** (aarch64): `vmovl_u16` + `vcvtq_f32_u32`
- 4 elements per iteration, rayon parallel chunks of 16K

### Bayer Demosaic (`bayer/`)
- **SSE3** / **NEON**: Load 9 neighbors (center + 4 cardinal + 4 diagonal), compute 4 interpolations in SIMD, scatter to RGB per-pixel
- **Parallel path** (images >= 128x128): Row-based rayon with per-row SIMD
- **Non-parallel path** (small images): Single-threaded full-image SIMD
- Scalar fallback for border pixels and non-SIMD architectures

### X-Trans Demosaic (`xtrans/`)
- No SIMD in normalization or demosaic kernel
- Rayon parallelism via row-based and `(direction, row)` flattening
- `UnsafeSendPtr` wrapper for Edition 2024 closure captures

## Test Coverage

**Total: 75+ tests** across all submodules.

| Area | Tests | Notes |
|------|-------|-------|
| `load_raw` error paths | 3 | Invalid path, invalid data, empty file |
| `load_raw` real data | 2 | Behind `real-data` feature flag |
| LibrawGuard RAII | 2 | Cleanup + null safety |
| Normalization | 2 | Small array + large array (SIMD+remainder) |
| CFA pattern | 6 | All 4 patterns, `red_in_row`, `pattern_2x2` |
| BayerImage validation | 5 | Zero dims, wrong length, margin overflow |
| Bayer demosaic | 18 | All patterns, uniform, gradient, corners, edges, channel preservation, NaN, SIMD-vs-scalar, parallel-vs-scalar |
| Scalar interpolation | 2 | Horizontal/vertical edge cases |
| X-Trans pattern | 3 | color_at, wrapping, invalid value panic |
| X-Trans image | 3 | Valid, zero width, wrong data length |
| process_xtrans | 4 | Output size, normalization, black clamp, full range |
| Hex lookup | 5 | Construction, offset range, sgrow, green neighbors, mod3 wrapping |
| Markesteijn | 5 | Output size, uniform, no-NaN, all-zeros, green preservation |
| Markesteijn steps | 5 | Green minmax, green interp, homogeneity, YPbPr |

## Benchmarks

Run with `LUMOS_CALIBRATION_DIR=<path> cargo test -p lumos --release <bench_name> -- --ignored --nocapture`:

- `bench_load_raw` - End-to-end loading time (file read + unpack + normalize + demosaic)
- `bench_load_raw_libraw_demosaic` - libraw's built-in demosaic at different quality levels
- `bench_markesteijn_quality_vs_libraw` - Quality comparison using linear regression per channel (removes WB/scale differences)

### Reference Numbers (X-Trans 6032x4028)
- Our Markesteijn 1-pass: ~480ms (vs libraw 1750ms single-threaded)
- Quality vs libraw 1-pass: MAE < 0.001, correlation ~0.96

## Current Issues

### 1. X-Trans normalization is scalar and single-threaded

**File**: `demosaic/xtrans/mod.rs:62-65`

X-Trans uses a scalar `.iter().map()` for u16-to-f32 normalization while Bayer uses `normalize_u16_to_f32_parallel()` (rayon + SIMD). For a 24MP X-Trans sensor, this is a missed optimization. Also uses `/range` instead of `*inv_range` (extra division per pixel).

**Fix**: Call `normalize_u16_to_f32_parallel()` from `normalize.rs`, same as Bayer path.

### 2. Bayer SIMD code duplication (4 copies)

The Bayer SIMD inner loop exists in 4 places:
- `bayer/mod.rs` lines 360-460 - parallel SSE3 row function
- `bayer/mod.rs` lines 485-580 - parallel NEON row function
- `bayer/simd_sse3.rs` - standalone SSE3 (non-parallel)
- `bayer/simd_neon.rs` - standalone NEON (non-parallel)

All four have identical logic: load 9 neighbors, compute h/v/cross/diag, extract to scalar, match on color pattern. The standalone files are only used for images < 128x128.

**Fix**: Either (a) extract the per-row SIMD into a shared function called by both paths, or (b) always use the parallel path since rayon handles small workloads with minimal overhead.

### 3. `std::mem::transmute` in standalone SIMD files

`simd_sse3.rs` and `simd_neon.rs` use `std::mem::transmute` to extract SIMD lanes:
```rust
let center_arr: [f32; 4] = std::mem::transmute(center);
```
The parallel row functions in `mod.rs` use the safer `_mm_storeu_ps` / `vst1q_f32`.

**Fix**: Replace `transmute` with `_mm_storeu_ps` / `vst1q_f32` for consistency and safety.

### 4. Missing `checked_mul` in `process_bayer_fast`

**File**: `mod.rs:359`

`process_monochrome` and X-Trans use `checked_mul().expect(...)` for pixel count, but `process_bayer_fast` uses bare `raw_width * raw_height`. Inconsistent defensive coding.

**Fix**: Add `checked_mul` for consistency.

### 5. Monochrome normalization uses `/range` instead of `*inv_range`

**File**: `mod.rs:325`

`process_monochrome` computes `((val as f32) - black).max(0.0) / range` per pixel. The Bayer path pre-computes `inv_range = 1.0 / range` and multiplies. Division is ~5x slower than multiplication.

**Fix**: Pre-compute `inv_range` and multiply, or better yet, call `normalize_u16_to_f32_parallel()` for the raw data and then extract the active region.

## Proposed Improvements

### Priority 1: Quick Wins (< 1 hour each)

1. **Use `normalize_u16_to_f32_parallel()` for X-Trans** - Reuse existing SIMD normalization. Estimated improvement: significant for file loading phase on X-Trans bodies (Fujifilm X-T5, X-H2, etc).

2. **Add `checked_mul` in `process_bayer_fast`** - One-line consistency fix.

3. **Replace `transmute` with store intrinsics** in `simd_sse3.rs` and `simd_neon.rs`.

4. **Use `*inv_range` in monochrome path** - Trivial perf improvement.

### Priority 2: Structural Cleanup (2-4 hours)

5. **Eliminate standalone SIMD files** (`simd_sse3.rs`, `simd_neon.rs`) - The non-parallel path for images < 128x128 can simply use the parallel path (rayon will run single-threaded for tiny workloads). Or extract per-row SIMD into shared functions. This removes ~400 lines of duplicated code and the associated maintenance burden.

### Priority 3: Performance Optimization (1-2 days)

6. **Monochrome: use `normalize_u16_to_f32_parallel` then crop** - Instead of interleaving normalization with crop, normalize the full raw buffer with SIMD, then extract the active region. Simpler code, better vectorization.

7. **X-Trans Markesteijn: SIMD for step 3+4 (RGB + derivatives)** - The R/B interpolation and YPbPr derivative computation are the most expensive steps. SIMD could accelerate the inner loops.

### Priority 4: Test Coverage Gaps

8. **Add test for `process_monochrome`** - Currently no unit test. Create a synthetic monochrome raw buffer and verify normalization + crop.

9. **Add test for `process_unknown_libraw_fallback`** - At minimum, test the 16-bit and 8-bit normalization branches.

10. **Add Bayer test combining non-trivial margins with non-RGGB patterns** - Current tests exercise margins and all CFA patterns separately, but not together.
