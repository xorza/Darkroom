# raw Module

## Overview

Loads RAW camera files via libraw FFI, detects sensor type (Mono/Bayer/XTrans/Unknown),
normalizes u16 sensor data to f32 [0,1], and dispatches to sensor-specific demosaicing.
Two entry points: `load_raw()` (demosaiced RGB) and `load_raw_cfa()` (un-demosaiced CFA
for calibration frames where hot pixel correction must precede demosaicing).

## Module Structure

```
raw/
  mod.rs              - LibrawGuard, BlackLevel, UnpackedRaw, consolidate_black_levels(),
                        compute_wb_multipliers(), apply_channel_corrections(), fc(),
                        open_raw(), load_raw(), load_raw_cfa()
  normalize.rs        - SIMD u16->f32 normalization (SSE2, SSE4.1, NEON, scalar)
  tests.rs            - Unit tests for loading, normalization, black level, WB, corrections
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
  -> consolidate_black_levels(cblack[4104], black, maximum, filters)
  -> compute_wb_multipliers(cam_mul[4])
  -> Monochrome:  normalize_u16(common_black) -> extract active area -> 1-channel output
  -> Bayer:       normalize_u16(common_black) -> apply_channel_corrections(delta+WB) -> demosaic_bayer() [todo!()]
  -> XTrans:      copy raw u16 -> drop libraw -> process_xtrans(channel_black, wb_mul)
  -> CFA (calib): normalize_u16(common_black) -> per-channel delta (NO WB) -> CfaImage
  -> Unknown:     libraw_dcraw_process fallback -> normalize 16/8-bit -> RGB output
```

### Black Level Consolidation
Replicates libraw's `adjust_bl()` from `utils_libraw.cpp:464-540`:
1. Fold spatial pattern (`cblack[4..5]` dimensions, `cblack[6..]` values) into per-channel `cblack[0..3]`
   - Bayer 2x2: map positions to color channels via FC macro, remap second green to G2
   - X-Trans 1x1: add `cblack[6]` to all 4 channels
2. Extract common minimum across channels, move to scalar `black`
3. Handle remaining spatial pattern (rare): extract common from pattern, move to `black`
4. Final: `per_channel[c] = cblack[c] + black`, `common = black`, `inv_range = 1/(max - common)`
5. `channel_delta_norm[c] = (per_channel[c] - common) * inv_range` for second-pass correction

### Two-Pass Normalization
- **Pass 1 (SIMD)**: `max(0, val - common_black) * inv_range` — existing SSE/NEON path, unchanged
- **Pass 2 (per-pixel)**: `(val - delta_norm[ch]).max(0.0) * wb_mul[ch]` via `apply_channel_corrections()`
  - Channel determined by `fc(filters, row, col)` — libraw FC macro
  - Parallelized with rayon `par_chunks_mut` by row
  - Skipped entirely if delta and WB are both trivial

### White Balance
- `compute_wb_multipliers(cam_mul)`: normalizes camera multipliers so min=1.0 (avoids clipping)
- `cam_mul[3]==0` for 3-color cameras → copies from `cam_mul[1]` (green)
- Returns `None` for invalid (zeros, negatives, NaN) → WB skipped
- CFA/calibration path: no WB applied (calibration frames need raw channel values)
- X-Trans: WB folded into `read_normalized()` per-pixel path (zero overhead)

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
- Per-channel black + WB folded into `read_normalized()` per-pixel path (zero overhead).
  `XTransImage` stores `channel_black: [f32; 3]` and `wb_mul: [f32; 3]` (R/G/B).

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

### Low: alloc_uninit_vec Safety
- 5 call sites, all with SAFETY comments explaining complete-write guarantees.
- Valid optimization for multi-megabyte buffers (avoids kernel page zeroing).
- Correctness depends on every parallel write path being exhaustive.

## Test Coverage

80+ tests across the module:
- Normalization: SIMD correctness, large arrays, below-black clamping, crop pattern
- Black level: consolidation (uniform, per-channel, Bayer 2x2 fold, X-Trans 1x1 fold)
- WB multipliers: normal, 3-channel, normalization, invalid (zeros, negative, NaN)
- Channel corrections: identity, delta-only, WB-only, negative clamping
- FC macro: RGGB pattern mapping, periodicity
- CFA patterns: all 4 Bayer variants, color_at, red_in_row, pattern_2x2
- BayerImage: validation panics (zero dims, wrong length, margin overflow)
- XTrans: pattern validation, image construction, output size, normalization, clamping,
  per-channel black + WB, u16/f32 path equivalence
- Markesteijn steps: green minmax, green interpolation, derivatives (uniform/checkerboard),
  homogeneity (uniform/dominant), YPbPr conversion, SAT queries, interior/border consistency,
  border no-panic, blend (uniform/dominant)
- Integration: load_raw with invalid/empty/valid files, dimension matching, guard cleanup
- Benchmarks: raw load timing, libraw quality levels, Markesteijn vs libraw (MAE/PSNR/correlation)

Real-data tests (`#[cfg_attr(not(feature = "real-data"), ignore)]`) require actual RAW files.
