# raw Module

## Overview

Loads RAW camera files via libraw FFI, detects sensor type (Mono/Bayer/XTrans/Unknown),
normalizes u16 sensor data to f32 [0,1], and dispatches to sensor-specific demosaicing.
Two entry points: `load_raw()` (demosaiced RGB) and `load_raw_cfa()` (un-demosaiced CFA
for calibration frames where hot pixel correction must precede demosaicing).

## Module Structure

```
raw/
  mod.rs              - LibrawGuard RAII, UnpackedRaw, open_raw(), load_raw(), load_raw_cfa()
  normalize.rs        - SIMD u16->f32 normalization (SSE2, SSE4.1, NEON, scalar)
  tests.rs            - 14 unit tests for loading, normalization, guard cleanup
  benches.rs          - Raw load benchmark, libraw quality comparison, Markesteijn vs libraw
  demosaic/
    mod.rs            - Re-exports CfaPattern, BayerImage, demosaic_bayer
    bayer/
      mod.rs          - CfaPattern enum, BayerImage struct, demosaic_bayer() [todo!()]
      tests.rs        - 11 tests for CFA patterns and BayerImage validation
    xtrans/
      mod.rs          - XTransPattern, XTransImage, process_xtrans() entry point
      markesteijn.rs  - DemosaicArena, 5-step orchestrator, unit tests
      markesteijn_steps.rs - Step implementations (green minmax, interpolate, derivatives,
                             homogeneity, blend), ColorInterpLookup, SAT queries, 20+ tests
      hex_lookup.rs   - HexLookup (3x3 repeating hex neighbor offsets), 6 tests
```

## Processing Pipeline

```
File -> libraw_open_buffer -> libraw_unpack -> detect_sensor_type(filters, colors)
  -> Monochrome:  normalize_u16_to_f32 -> extract active area -> 1-channel output
  -> Bayer:       normalize_u16_to_f32 -> demosaic_bayer() [todo!() - panics]
  -> XTrans:      copy raw u16 -> drop libraw -> process_xtrans (normalizes on-the-fly)
  -> Unknown:     libraw_dcraw_process fallback -> normalize 16/8-bit -> RGB output
```

### Normalization Formula
`output = max(0, (value_u16 as f32 - black)) * inv_range` where `inv_range = 1/(maximum - black)`.
Values above 1.0 are preserved (no upper clamp). Parallelized via rayon with 16K-element chunks.

### CFA Pattern Detection
`detect_sensor_type(filters, colors)` in `astro_image/sensor.rs`:
- `filters == 0 || colors == 1` -> Monochrome
- `filters == 9` -> XTrans (libraw convention)
- Bayer: decodes filters bitmask to RGGB/BGGR/GRBG/GBRG
- Otherwise: Unknown (falls back to libraw demosaic)

## Bayer Demosaic Status

**Not implemented.** `demosaic_bayer()` in `bayer/mod.rs:157` contains `todo!("Bayer DCB
demosaicing not yet implemented")`. Any Bayer camera file (>95% of cameras) will panic.

The `CfaPattern` enum and `BayerImage` struct are complete with full validation and tests.
`color_at()`, `red_in_row()`, `pattern_2x2()` are all implemented and tested for all 4 patterns.

### Industry Recommendations for Bayer Demosaic
- **RCD** (Ratio Corrected Demosaicing): Default in Siril. Best for round objects like stars.
  Reduces color overshooting common in other methods. Good speed/quality balance.
- **AMaZE** (Aliasing Minimization and Zipper Elimination): Best detail recovery, very slow.
  Preferred for low-ISO. Used by RawTherapee for Bayer sensors.
- **DCB**: Better at avoiding false colors on cameras without AA filters.
- **Bilinear**: Simplest, fast, low quality. Adequate as a baseline fallback.
- libraw quality levels: 0=linear, 1=VNG, 2=PPG, 3=AHD, 11=DHT, 12=AAHD.
- For astrophotography, **RCD is the recommended first implementation** due to its
  superior handling of star profiles with manageable complexity.

## X-Trans Markesteijn Implementation

### Algorithm (1-pass, 4 directions)
1. **Green min/max**: For non-green pixels, scan 6 hex neighbors to bound green range.
2. **Green interpolation**: 4 directional estimates using weighted hex formulas, clamped to bounds.
3. **Derivatives**: YPbPr spatial Laplacian per direction (RGB recomputed on-the-fly).
4. **Homogeneity**: 3x3 window counting pixels where `drv <= 8 * min_drv`.
5. **Blend**: 5x5 SAT-based homogeneity query, average qualifying directions' RGB (on-the-fly).

### Memory Layout (Arena)
```
10P f32 total, where P = width * height
Region A [0..4P]:   green_dir (4 directions)     - Steps 2-5
Region B [4P..8P]:  drv (Steps 3-4) / output RGB (Step 5, first 3P)
Region C [8P..9P]:  gmin (Steps 1-2) -> homo as u8 (Steps 4-5, reinterpret cast)
Region D [9P..10P]: gmax (Steps 1-2) -> threshold (Step 4)
```

RGB is never materialized as a buffer -- recomputed on-the-fly from green_dir to avoid
the 12P rgb_dir buffer (~1.1 GB for 6032x4028). Peak arena: ~920 MB for full-res X-Trans.

### Performance Optimizations
- `ColorInterpLookup`: Precomputed 6x6x2 neighbor strategies (Pair/Single/None).
- Interior fast path: skips bounds checks for ~99% of pixels.
- Sliding 3-row YPbPr cache in derivatives (3x fewer `compute_rgb_pixel` calls).
- SATs built one direction at a time (1P peak instead of 4P).
- Rayon parallelism at row/chunk level in all 5 steps.
- U16 raw data kept until demosaic (half the memory of pre-normalizing to f32).
- Libraw and file buffer dropped before demosaic to reduce peak memory by ~77 MB.

### Correctness Assessment
- Hex lookup construction matches dcraw/libraw's allhex logic faithfully.
- Green interpolation weights (0.6796875, 0.87109375, etc.) match reference coefficients.
- YPbPr uses BT.2020 coefficients (0.2627, 0.6780, 0.0593) matching reference. BT.709 vs
  BT.2020 makes negligible difference since it is a relative metric for direction selection.
- Homogeneity threshold (8x min) matches reference implementation.
- Blend uses 5x5 window with 7/8 max threshold, matching reference.
- Quality benchmark confirms comparable MAE/PSNR vs libraw 1-pass reference.
- 2.1x speedup over libraw's single-threaded Markesteijn (multi-threaded via rayon).

### Known Limitation: No 3-pass Mode
Only 1-pass (4 directions). Reference also has 3-pass (8 directions with median refinement)
for slightly better quality at ~3x cost. 1-pass is sufficient for astrophotography workflows.

## SIMD Normalization Coverage

| Architecture | Instruction Set | Elements/Iteration | Notes |
|---|---|---|---|
| x86_64 | SSE4.1 | 4 (128-bit) | Preferred: `_mm_cvtepu16_epi32` (pmovzxwd) |
| x86_64 | SSE2 | 4 (128-bit) | Fallback: `_mm_unpacklo_epi16` + zero |
| aarch64 | NEON | 4 (128-bit) | Always available on aarch64 |
| Other | Scalar | 1 | Loop fallback |

**Missing: AVX2 path** (8 elements/iteration, 256-bit). Would use `_mm256_cvtepu16_epi32`
for 2x throughput on x86_64. SSE paths process 4 elements at a time; AVX2 would double that.
The project uses AVX2 intrinsics elsewhere, so the infrastructure exists.

Runtime feature detection uses `common::cpu_features::has_sse4_1()` / `has_sse2()`.

## Issues and Gaps

### Critical: Bayer Demosaic Panics
- `demosaic_bayer()` is `todo!()` -- any Bayer RAW file crashes the application.
- Workaround: could route Bayer to `demosaic_libraw_fallback()` as interim fix.
- Long-term: implement RCD for astrophotography-quality demosaicing.

### Medium: Missing Per-Channel Black Level (cblack[])
- Uses only `color.black` (scalar). Ignores `cblack[0..3]` per-channel corrections
  and `cblack[4..5]` spatial pattern (6x6 for X-Trans, 2x2 for some Bayer sensors).
- Correct formula per libraw docs: `black_for_pixel = black + cblack[channel] + cblack[pattern]`.
- Sony, some Canon, and X-Trans cameras have non-trivial cblack values.
- Impact: subtle color bias in shadows and calibration frames.
- Alternative: call `libraw_subtract_black()` before reading raw_image to let libraw
  handle the full black level model, then normalize from 0 to adjusted maximum.

### Medium: No Pre-Demosaic White Balance
- Standard RAW pipeline: black subtraction -> white balance -> demosaic -> color correction.
- This implementation skips white balance entirely (no `cam_mul`/`pre_mul` usage).
- For astrophotography this is defensible (white balance applied later in the pipeline),
  but it can degrade demosaic quality at color transitions and in the libraw fallback path.
- The libraw fallback path does apply `use_camera_wb = 1`, creating an inconsistency
  between the custom demosaic paths and the fallback path.
- Quality comparison benchmark uses linear regression to remove scale/offset differences,
  which effectively compensates for missing white balance in the comparison.

### Low: No Upper Clamp in Normalization
- Values above `maximum` produce output > 1.0. Tests explicitly validate this behavior.
- Industry standard (libraw, RawTherapee, dcraw) clips at white point.
- Hot pixels and saturated star cores can produce values >> 1.0, which may cause
  artifacts in downstream processing (e.g., X-Trans demosaic interpolation overshoot).

### Low: load_raw_cfa Panics on Unknown Sensor
- `unimplemented!()` at mod.rs:535 for Unknown sensor type.
- Per project error handling rules, expected failures should return `Result::Err`.

### Low: alloc_uninit_vec Safety
- 5 call sites, all with SAFETY comments explaining complete-write guarantees.
- Valid optimization for multi-megabyte buffers (avoids kernel page zeroing).
- Correctness depends on every parallel write path being exhaustive.

## Test Coverage

67+ tests across the module:
- Normalization: SIMD correctness, large arrays, below-black clamping, crop pattern
- CFA patterns: all 4 Bayer variants, color_at, red_in_row, pattern_2x2
- BayerImage: validation panics (zero dims, wrong length, margin overflow)
- XTrans: pattern validation, image construction, output size, normalization, clamping
- Markesteijn steps: green minmax, green interpolation, derivatives (uniform/checkerboard),
  homogeneity (uniform/dominant), YPbPr conversion, SAT queries, interior/border consistency,
  border no-panic, blend (uniform/dominant)
- Integration: load_raw with invalid/empty/valid files, dimension matching, guard cleanup
- Benchmarks: raw load timing, libraw quality levels, Markesteijn vs libraw (MAE/PSNR/correlation)

Real-data tests (`#[cfg_attr(not(feature = "real-data"), ignore)]`) require actual RAW files.
