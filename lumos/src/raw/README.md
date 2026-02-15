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
    │   ├── mod.rs       # CfaPattern, BayerImage types + demosaic_bayer() stub
    │   └── tests.rs     # CFA pattern and BayerImage validation tests
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
| Monochrome | `normalize_u16_to_f32_parallel()` + crop | None | 1-channel grayscale |
| Bayer | `normalize_u16_to_f32_parallel()` (SIMD) | RCD (Ratio Corrected Demosaicing) | 3-channel RGB |
| X-Trans | `normalize_u16_to_f32_parallel()` (SIMD) | Markesteijn 1-pass (rayon) | 3-channel RGB |
| Unknown | libraw `dcraw_process` | libraw built-in | 3-channel RGB |

## SIMD Strategy

### Normalization (`normalize.rs`)
- **SSE4.1** (preferred): `_mm_cvtepu16_epi32` for u16 zero-extension
- **SSE2** (fallback): `_mm_unpacklo_epi16` with zero register
- **NEON** (aarch64): `vmovl_u16` + `vcvtq_f32_u32`
- 4 elements per iteration, rayon parallel chunks of 16K
- Used by Monochrome, Bayer, and X-Trans paths

### X-Trans Demosaic (`xtrans/`)
- Precomputed `ColorInterpLookup` (fixed-size arrays, no heap alloc) for neighbor pattern
- Interior fast path for `interpolate_missing_color_fast` (skips bounds checks for ~99% of pixels)
- Summed area tables (SAT) for O(1) 5x5 window queries in blend step
- Uninitialized buffer allocation (`alloc_uninit_vec`) for all large intermediate arrays
- Buffer reuse: `gmin` reinterpreted as `homo`, `drv` reused as blend output
- Rayon parallelism via row-based and `(direction, row)` flattening
- `UnsafeSendPtr` wrapper for Edition 2024 closure captures

## Memory Optimization (X-Trans 6032x4028)

Peak ~2.3GB for high-quality 4-direction interpolation. Buffer lifecycle:

| Buffer | Size | Lifetime | Notes |
|--------|------|----------|-------|
| `green_dir` | 384 MB | Steps 2-3 | Freed after RGB computed |
| `rgb_dir` | 1,152 MB | Steps 3-6 | Largest; holds 4-dir RGB |
| `gmin`/`gmax` | 96 MB each | Steps 1-2 | `gmin` reused as `homo` |
| `drv` | 384 MB | Steps 3-6 | Reused as blend output buffer |
| `homo` | 96 MB | Steps 5-6 | Reinterpreted from `gmin` memory |
| SAT tables | 96 MB | Step 6 only | 4 summed area tables, uninit alloc |

All large buffers use `alloc_uninit_vec` to skip kernel page zeroing (`clear_page_erms`).

## Test Coverage

**Total: 67 tests** across all submodules.

| Area | Tests | Notes |
|------|-------|-------|
| `load_raw` error paths | 3 | Invalid path, invalid data, empty file |
| `load_raw` real data | 2 | Behind `real-data` feature flag |
| LibrawGuard RAII | 2 | Cleanup + null safety |
| Normalization | 5 | Small/large array, below-black clamping, monochrome crop, fallback formulas |
| CFA pattern | 6 | All 4 patterns, `red_in_row`, `pattern_2x2` |
| BayerImage validation | 6 | Zero dims, wrong length, margin overflow, valid construction |
| X-Trans pattern | 3 | color_at, wrapping, invalid value panic |
| X-Trans image | 3 | Valid, zero width, wrong data length |
| process_xtrans | 4 | Output size, normalization, black clamp, full range |
| Hex lookup | 5 | Construction, offset range, sgrow, green neighbors, mod3 wrapping |
| Markesteijn | 5 | Output size, uniform, no-NaN, all-zeros, green preservation |
| Markesteijn steps | 7 | Green minmax (2), green interp, homogeneity, YPbPr (2), green clamping |
| SAT (summed area table) | 6 | Uniform, sequential, single pixel/row/column, zeros |
| Homogeneity | 2 | Border pixels zero, dominant direction scoring |
| ColorInterpLookup | 2 | Coverage of all positions, pair symmetry |
| Interpolation paths | 2 | Interior-vs-border consistency, border no-panic |
| Blend | 2 | Uniform homo averaging, dominant direction selection |

## Benchmarks

Run with `LUMOS_CALIBRATION_DIR=<path> cargo test -p lumos --release <bench_name> -- --ignored --nocapture`:

- `raw_load` - End-to-end loading time (file read + unpack + normalize + demosaic)
- `bench_load_raw_libraw_demosaic` - libraw's built-in demosaic at different quality levels
- `bench_markesteijn_quality_vs_libraw` - Quality comparison using linear regression per channel (removes WB/scale differences)

### Reference Numbers (X-Trans 6032x4028)
- Our Markesteijn 1-pass: ~1238ms total / ~425ms demosaic (vs libraw ~2500ms total)
- Quality vs libraw 1-pass: MAE ~0.0005, avg correlation ~0.91 (R=0.89, G=0.96, B=0.88)
- Quality vs libraw 3-pass: MAE ~0.0005 (libraw 1-pass vs 3-pass baseline: MAE ~0.0003)
- Speedup vs libraw 1-pass: 2.1x
