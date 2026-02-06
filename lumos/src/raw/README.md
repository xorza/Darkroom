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
    │   ├── mod.rs       # Dispatcher + SIMD row functions (SSE3/NEON)
    │   ├── scalar.rs    # Scalar bilinear interpolation functions
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
| X-Trans | `normalize_u16_to_f32_parallel()` (SIMD) | Markesteijn 1-pass (rayon) | 3-channel RGB |
| Unknown | libraw `dcraw_process` | libraw built-in | 3-channel RGB |

## SIMD Strategy

### Normalization (`normalize.rs`)
- **SSE4.1** (preferred): `_mm_cvtepu16_epi32` for u16 zero-extension
- **SSE2** (fallback): `_mm_unpacklo_epi16` with zero register
- **NEON** (aarch64): `vmovl_u16` + `vcvtq_f32_u32`
- 4 elements per iteration, rayon parallel chunks of 16K
- Used by both Bayer and X-Trans paths

### Bayer Demosaic (`bayer/`)
- **SSE3** / **NEON**: Load 9 neighbors (center + 4 cardinal + 4 diagonal), compute 4 interpolations in SIMD, assign via shared `assign_simd_pixels` helper
- Row-based rayon parallelism with per-row SIMD
- Shared `demosaic_pixel_scalar` helper for border/tail pixels and scalar fallback
- Scalar fallback for non-SIMD architectures

### X-Trans Demosaic (`xtrans/`)
- Precomputed `ColorInterpLookup` for neighbor pattern (avoids per-pixel pattern search)
- Uninitialized buffer allocation for intermediate arrays (avoids kernel page zeroing)
- Rayon parallelism via row-based and `(direction, row)` flattening
- `UnsafeSendPtr` wrapper for Edition 2024 closure captures

## Test Coverage

**Total: 75+ tests** across all submodules.

| Area | Tests | Notes |
|------|-------|-------|
| `load_raw` error paths | 3 | Invalid path, invalid data, empty file |
| `load_raw` real data | 2 | Behind `real-data` feature flag |
| LibrawGuard RAII | 2 | Cleanup + null safety |
| Normalization | 5 | Small array, large array, below-black clamping, monochrome crop pattern, fallback normalization formulas |
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

- `raw_load` - End-to-end loading time (file read + unpack + normalize + demosaic)
- `bench_load_raw_libraw_demosaic` - libraw's built-in demosaic at different quality levels
- `bench_markesteijn_quality_vs_libraw` - Quality comparison using linear regression per channel (removes WB/scale differences)

### Reference Numbers (X-Trans 6032x4028)
- Our Markesteijn 1-pass: ~1273ms best (vs libraw ~2500ms)
- Quality vs libraw 1-pass: MAE ~0.000521, correlation ~0.96


