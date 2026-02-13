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

### Comparison with Industry Standard Pipeline

The industry standard raw processing pipeline (libraw/dcraw/RawTherapee) is:

1. **Black level subtraction** (per-channel + spatial pattern)
2. **White balance** (scale channels by camera WB multipliers)
3. **Demosaic** (interpolate missing color channels)
4. **Color matrix** (cam_xyz or rgb_cam: camera color space -> XYZ -> sRGB)
5. **Gamma / tone curve** (linear -> perceptual encoding)
6. **Highlight recovery** (optional, before demosaic in some implementations)

This implementation covers steps 1-3. Steps 4-5 are intentionally omitted because the
astrophotography workflow operates in linear camera-native color space. Color matrix
conversion is not needed for stacking/calibration and would introduce unnecessary
interpolation error. Gamma is applied later in the display pipeline.

**Pipeline order is correct**: black subtraction -> WB -> demosaic matches the standard.
Some processors (darktable, Lightroom) apply WB after demosaic for speed, but pre-demosaic
WB generally produces slightly better results since the demosaic algorithm sees balanced
channel values. This implementation correctly applies WB before demosaic for both Bayer
and X-Trans paths.

### Black Level Consolidation

Replicates libraw's `adjust_bl()` from `utils_libraw.cpp:464-540`:
1. Fold spatial pattern (`cblack[4..5]` dimensions, `cblack[6..]` values) into per-channel `cblack[0..3]`
   - Bayer 2x2: map positions to color channels via FC macro, remap second green to G2
   - X-Trans 1x1: add `cblack[6]` to all 4 channels
2. Extract common minimum across channels, move to scalar `black`
3. Handle remaining spatial pattern (rare): extract common from pattern, move to `black`
4. Final: `per_channel[c] = cblack[c] + black`, `common = black`, `inv_range = 1/(max - common)`
5. `channel_delta_norm[c] = (per_channel[c] - common) * inv_range` for second-pass correction

**Assessment vs professional processors**: The consolidation logic faithfully replicates
libraw's `adjust_bl()`. Handles the three main cases: uniform black, per-channel corrections,
and spatial patterns. The two-pass approach (SIMD common + per-pixel delta) is an efficient
optimization over the single-pass per-pixel approach used by libraw/dcraw.

**Limitation**: Does not use masked/optical-black pixel areas to compute black levels.
Some cameras (Canon) have optically masked pixels at sensor edges that provide a more
accurate real-time black level measurement. LibRaw itself calculates black from masked
pixels when available. This implementation relies entirely on libraw's computed `black`
and `cblack` values, which already incorporate masked-pixel data when libraw computes it.
So the limitation is theoretical -- in practice, libraw handles it before we see the values.

### Two-Pass Normalization

- **Pass 1 (SIMD)**: `max(0, val - common_black) * inv_range` -- existing SSE/NEON path, unchanged
- **Pass 2 (per-pixel)**: `(val - delta_norm[ch]).max(0.0) * wb_mul[ch]` via `apply_channel_corrections()`
  - Channel determined by `fc(filters, row, col)` -- libraw FC macro
  - Parallelized with rayon `par_chunks_mut` by row
  - Skipped entirely if delta and WB are both trivial

**Normalization formula**: `clamp((val - black) / (maximum - black), 0, 1)` is correct and
matches the standard. The two-pass split is a valid optimization: SIMD handles the bulk
(uniform black), per-pixel handles the per-channel residual.

### White Balance

- `compute_wb_multipliers(cam_mul)`: normalizes camera multipliers so min=1.0 (avoids clipping)
- `cam_mul[3]==0` for 3-color cameras -> copies from `cam_mul[1]` (green)
- Returns `None` for invalid (zeros, negatives, NaN) -> WB skipped
- CFA/calibration path: no WB applied (calibration frames need raw channel values)
- X-Trans: WB folded into `read_normalized()` per-pixel path (zero overhead)

**Assessment**: Normalization to min=1.0 is a deliberate choice that avoids clipping green
(the reference channel) but may clip highlights in red/blue channels when WB multipliers
are large (e.g., tungsten light where blue multiplier can exceed 3.0). This is the same
approach used by dcraw. Professional processors (RawTherapee, darktable) normalize to the
green channel specifically (`mul[c] /= mul[1]`) rather than the minimum, which gives
slightly different behavior when green is not the smallest channel. For astrophotography
the difference is negligible since sky backgrounds are neutral and stars are broadband.

**Missing: daylight multipliers / user WB selection**. Only camera-embedded WB (`cam_mul`)
is supported. No option for daylight, shade, fluorescent, or custom color temperature.
For astro work, camera WB is generally adequate since the primary concern is calibration
frame consistency, not absolute color accuracy.

### CFA Pattern Detection

`detect_sensor_type(filters, colors)` in `astro_image/sensor.rs`:
- `filters == 0 || colors == 1` -> Monochrome
- `filters == 9` -> XTrans (libraw convention)
- Bayer: decodes filters bitmask to RGGB/BGGR/GRBG/GBRG
- Otherwise: Unknown (falls back to libraw demosaic)

## Bayer Demosaic Status

**Not implemented.** `demosaic_bayer()` in `bayer/mod.rs:189` contains `todo!("Bayer DCB
demosaicing not yet implemented")`. Any Bayer camera file (>95% of cameras) will panic.

The `CfaPattern` enum and `BayerImage` struct are complete with full validation and tests.
`color_at()`, `red_in_row()`, `pattern_2x2()`, `flip_vertical()`, `flip_horizontal()`,
`from_bayerpat()` are all implemented and tested for all 4 patterns.

### Industry Bayer Demosaic Algorithm Comparison

| Algorithm | Quality | Speed | Best For | Notes |
|-----------|---------|-------|----------|-------|
| **RCD** | High | Fast | Stars, round edges | Default in Siril. Reduces color overshooting. |
| **AMaZE** | Highest | Slow | Low-ISO detail | Default in RawTherapee. Best fine detail recovery. |
| **DCB** | High | Medium | No-AA-filter cameras | Good at avoiding false colors. |
| **VNG4** | Medium | Medium | Low-contrast, sky | Handles crosstalk well. Loses fine detail. |
| **AHD** | Medium | Slow | General | Widely used but old, generally inferior to AMaZE/RCD. |
| **LMMSE/IGV** | High | Medium | High-ISO noise | Best for noisy images. IGV mitigates moire. |
| **Bilinear** | Low | Fast | Previews only | Simplest. Adequate as baseline fallback. |
| libraw built-in | Variable | Variable | Fallback | 0=linear, 1=VNG, 2=PPG, 3=AHD, 11=DHT, 12=AAHD |

**Recommendation for implementation**: RCD (Ratio Corrected Demosaicing) is the recommended
first implementation. Four-step algorithm: (1) directional discrimination via 1D gradients,
(2) low-pass smoothing with 3x3 kernel, (3) green interpolation with ratio correction,
(4) red/blue interpolation via color differences. Reference C implementation available at
github.com/LuisSR/RCD-Demosaicing. Manageable complexity, fast, excellent star handling.

Dual demosaic (RCD + VNG4) would be ideal: RCD for high-detail areas, VNG4 for smooth
sky regions. RawTherapee and darktable both support this mode.

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

### Quality Assessment vs libraw Reference
- Hex lookup construction matches dcraw/libraw's allhex logic faithfully.
- Green interpolation weights (0.6796875, 0.87109375, etc.) match reference coefficients.
- YPbPr uses BT.2020 coefficients (0.2627, 0.6780, 0.0593) matching reference. BT.709 vs
  BT.2020 makes negligible difference since it is a relative metric for direction selection.
- Homogeneity threshold (8x min) matches reference implementation.
- Blend uses 5x5 window with 7/8 max threshold, matching reference.
- Quality benchmark: MAE ~0.0005, avg correlation ~0.91 (R=0.89, G=0.96, B=0.88) vs
  libraw 1-pass. For reference, libraw's own 1-pass vs 3-pass differs by MAE ~0.0003.
- The lower red/blue correlation (0.88-0.89 vs green's 0.96) is expected because green
  is directly measured at ~55% of X-Trans pixels while red/blue rely on interpolation.
- 2.1x speedup over libraw's single-threaded Markesteijn (multi-threaded via rayon).
- Performance: ~620ms demosaic for 6032x4028 (vs libraw ~1750ms single-threaded).

### Known Limitation: No 3-pass Mode
Only 1-pass (4 directions). Reference also has 3-pass (8 directions with median refinement)
for slightly better quality at ~3x cost. Per darktable/RawTherapee users, the quality
difference is only visible in low-ISO shots. 1-pass is sufficient for astrophotography
workflows where images are stacked.

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

## Missing Pipeline Steps (Comparison with Professional Processors)

### Not Implemented -- By Design

| Step | RawTherapee | darktable | libraw | This impl | Reason |
|------|-------------|-----------|--------|-----------|--------|
| Color matrix (cam->sRGB) | Yes | Yes | Yes | No | Astro works in camera-native linear space |
| Gamma / tone curve | Yes | Yes | Yes | No | Applied later in display pipeline |
| Highlight recovery | Yes | Yes | No | No | Not needed for astro (no blown highlights) |
| Output color space | Yes | Yes | Yes | No | sRGB/ProPhoto conversion done downstream |

### Not Implemented -- Could Add Value

| Step | Impact | Effort | Notes |
|------|--------|--------|-------|
| **Bayer demosaic (RCD)** | Critical | Medium | Without this, >95% of cameras panic |
| **Hot pixel removal** | High | Low | Some hot pixels survive dark subtraction |
| **Chromatic aberration** | Medium | Medium | Lateral CA visible on fast optics |
| **Lens vignetting** | Low | Low | Usually handled by flat-field calibration |
| **Noise reduction** | Low | High | Better done post-stack in astro workflow |
| **Lens distortion** | Low | Medium | Handled by registration/warp module |
| **AVX2 normalization** | Low | Low | 2x SIMD throughput, normalization is not bottleneck |
| **3-pass Markesteijn** | Low | Medium | Minimal quality gain for astro (stacking averages) |

## Issues and Gaps

### Critical: Bayer Demosaic Panics
- `demosaic_bayer()` is `todo!()` -- any Bayer RAW file crashes the application.
- Workaround: could route Bayer to `demosaic_libraw_fallback()` as interim fix.
- Long-term: implement RCD for astrophotography-quality demosaicing.

### Low: alloc_uninit_vec Safety
- 5 call sites, all with SAFETY comments explaining complete-write guarantees.
- Valid optimization for multi-megabyte buffers (avoids kernel page zeroing).
- Correctness depends on every parallel write path being exhaustive.

### Low: Libraw Fallback Normalization
- The `demosaic_libraw_fallback()` path normalizes 16-bit output by dividing by 65535.0,
  which assumes the full u16 range. LibRaw's processed output may not actually use the
  full range depending on camera and processing settings. This could produce slightly
  compressed dynamic range in the fallback path. Not critical since this path is only
  used for unknown/exotic sensors.

## Test Coverage

80+ tests across the module:
- Normalization: SIMD correctness, large arrays, below-black clamping, crop pattern
- Black level: consolidation (uniform, per-channel, Bayer 2x2 fold, X-Trans 1x1 fold)
- WB multipliers: normal, 3-channel, normalization, invalid (zeros, negative, NaN)
- Channel corrections: identity, delta-only, WB-only, combined, negative clamping
- FC macro: RGGB pattern mapping, periodicity
- CFA patterns: all 4 Bayer variants, color_at, red_in_row, pattern_2x2, flip_v/h, from_bayerpat
- BayerImage: validation panics (zero dims, wrong length, margin overflow)
- XTrans: pattern validation, image construction, output size, normalization, clamping,
  per-channel black + WB, u16/f32 path equivalence
- Markesteijn steps: green minmax, green interpolation, derivatives (uniform/checkerboard),
  homogeneity (uniform/dominant/border), YPbPr conversion (white/primary/black), SAT queries,
  interior/border consistency, border no-panic, blend (uniform/dominant)
- Integration: load_raw with invalid/empty/valid files, dimension matching, guard cleanup
- Benchmarks: raw load timing, libraw quality levels, Markesteijn vs libraw (MAE/PSNR/correlation)

Real-data tests (`#[cfg_attr(not(feature = "real-data"), ignore)]`) require actual RAW files.

## Benchmarks

Run with `LUMOS_CALIBRATION_DIR=<path> cargo test -p lumos --release <bench_name> -- --ignored --nocapture`:

- `raw_load` -- End-to-end loading time (file read + unpack + normalize + demosaic)
- `bench_load_raw_libraw_demosaic` -- libraw's built-in demosaic at different quality levels
- `bench_markesteijn_quality_vs_libraw` -- Quality comparison using linear regression per channel

### Reference Numbers (X-Trans 6032x4028)
- Our Markesteijn 1-pass: ~1238ms total / ~620ms demosaic (vs libraw ~2500ms total)
- Quality vs libraw 1-pass: MAE ~0.0005, avg correlation ~0.91 (R=0.89, G=0.96, B=0.88)
- Quality vs libraw 3-pass: MAE ~0.0005 (libraw 1-pass vs 3-pass baseline: MAE ~0.0003)
- Speedup vs libraw 1-pass: 2.1x
