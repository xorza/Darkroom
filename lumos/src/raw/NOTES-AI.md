# raw Module

## Module Overview

Loads RAW camera files via libraw FFI, detects sensor type (Mono/Bayer/XTrans/Unknown),
normalizes u16 sensor data to f32 [0,1], and dispatches to sensor-specific demosaicing.
Two entry points: `load_raw()` (demosaiced RGB) and `load_raw_cfa()` (un-demosaiced CFA
for calibration frames where hot pixel correction must precede demosaicing).

See subdirectory NOTES-AI.md files for detailed analysis:
- `demosaic/xtrans/NOTES-AI.md` -- X-Trans Markesteijn implementation review
- `demosaic/bayer/NOTES-AI.md` -- RCD implementation details and benchmarks

### Module Structure

```
raw/
  mod.rs              - LibrawGuard, BlackLevel, UnpackedRaw, consolidate_black_levels(),
                        compute_wb_multipliers(), apply_channel_corrections(), fc(),
                        open_raw(), load_raw(), load_raw_cfa(), extract_iso()
  normalize.rs        - SIMD u16->f32 normalization (SSE2, SSE4.1, NEON, scalar)
  tests.rs            - Unit tests for loading, normalization, black level, WB, corrections
  benches.rs          - Raw load benchmark, libraw quality comparison, Markesteijn vs libraw
  demosaic/
    mod.rs            - Re-exports CfaPattern, BayerImage, demosaic_bayer
    bayer/
      mod.rs          - CfaPattern enum, BayerImage struct, demosaic_bayer() entry point
      rcd.rs          - RCD algorithm (5 steps, rayon row-parallel, ~650 lines)
      tests.rs        - 20 tests (11 CFA pattern + 9 RCD correctness)
    xtrans/
      mod.rs          - XTransPattern, XTransImage, PixelSource, process_xtrans(),
                        process_xtrans_f32() entry points
      markesteijn.rs  - DemosaicArena, 5-step orchestrator, unit tests
      markesteijn_steps.rs - Step implementations (green minmax, interpolate, derivatives,
                             homogeneity, blend), ColorInterpLookup, SAT queries, 20+ tests
      hex_lookup.rs   - HexLookup (3x3 repeating hex neighbor offsets), 6 tests
```

### Error Handling

All public functions (`open_raw`, `load_raw`, `load_raw_cfa`) return
`Result<..., ImageLoadError>`. Libraw FFI failures map to `ImageLoadError::Raw { path, reason }`;
file I/O errors map to `ImageLoadError::Io { path, source }`.

### Key Types

- `LibrawGuard` / `ProcessedImageGuard` -- RAII for libraw pointers
- `BlackLevel` -- Consolidated per-channel black with common/delta/inv_range for two-pass normalization
- `UnpackedRaw` -- Intermediate state after `libraw_unpack`, methods for each sensor path
  (carries `path: PathBuf` for error reporting)
- `CfaPattern` -- Enum (Rggb/Bggr/Grbg/Gbrg), pattern queries, flip ops, FITS parsing
- `BayerImage` -- Borrowed view into normalized Bayer data with margins and CFA pattern
- `XTransPattern` -- 6x6 pattern wrapper with `color_at()`
- `XTransImage` -- Raw data + margins + normalization params, `PixelSource::U16`/`F32` dual path
- `DemosaicArena` -- Single contiguous 10P f32 allocation with region reuse

## Pipeline Analysis

### Processing Pipeline

```
File -> libraw_open_buffer -> libraw_unpack -> detect_sensor_type(filters, colors)
  -> consolidate_black_levels(cblack[4104], black, maximum, filters)
  -> compute_wb_multipliers(cam_mul[4])
  -> Monochrome:  normalize_u16(common_black) -> extract active area -> 1-channel output
  -> Bayer:       normalize_u16(common_black) -> apply_channel_corrections(delta+WB) -> demosaic_bayer(RCD)
  -> XTrans:      copy raw u16 -> drop libraw -> process_xtrans(channel_black, wb_mul)
  -> CFA (calib): normalize_u16(common_black) -> per-channel delta (NO WB) -> CfaImage
  -> Unknown:     libraw_dcraw_process fallback -> normalize 16/8-bit -> RGB output
```

### Industry Standard Pipeline Comparison

The industry standard raw processing pipeline (libraw/dcraw/RawTherapee/darktable) is:

1. **Black level subtraction** (per-channel + spatial pattern)
2. **Hot pixel correction** (optional, on raw CFA data)
3. **Raw chromatic aberration correction** (optional, pre-demosaic -- darktable does this)
4. **White balance** (scale channels by camera WB multipliers)
5. **Demosaic** (interpolate missing color channels)
6. **Color matrix** (cam_xyz or rgb_cam: camera color space -> XYZ -> sRGB)
7. **Gamma / tone curve** (linear -> perceptual encoding)

This implementation covers steps 1, 4, 5. Steps 6-7 are intentionally omitted for the
astrophotography workflow (linear camera-native color space). Color matrix conversion is
not needed for stacking/calibration. Gamma is applied later in the display pipeline.

**Pipeline order is correct**: black subtraction -> WB -> demosaic matches the standard.
Pre-demosaic WB generally produces slightly better results since the demosaic algorithm
sees balanced channel values. This implementation correctly applies WB before demosaic
for both Bayer and X-Trans paths.

### Black Level Consolidation

Replicates libraw's `adjust_bl()` from `utils_libraw.cpp:464-540`:
1. Fold spatial pattern into per-channel `cblack[0..3]` (Bayer 2x2 / X-Trans 1x1)
2. Extract common minimum across channels, move to scalar `black`
3. Handle remaining spatial pattern (rare)
4. Final: `per_channel[c] = cblack[c] + black`, `inv_range = 1/(max - common)`
5. `channel_delta_norm[c] = (per_channel[c] - common) * inv_range`

Two-pass normalization: SIMD pass applies common black, per-pixel pass applies channel delta + WB.

### White Balance

- `compute_wb_multipliers(cam_mul)`: normalizes so min=1.0 (avoids clipping, same as dcraw)
- `cam_mul[3]==0` -> copies from `cam_mul[1]` (green)
- Returns `None` for invalid -> WB skipped
- CFA/calibration path: no WB applied
- X-Trans: WB folded into `read_normalized()` per-pixel path

### SIMD Normalization

| Architecture | Instruction Set | Elements/Iteration |
|---|---|---|
| x86_64 | SSE4.1 | 4 (128-bit, `_mm_cvtepu16_epi32`) |
| x86_64 | SSE2 | 4 (128-bit, `_mm_unpacklo_epi16`) |
| aarch64 | NEON | 4 (128-bit) |
| Other | Scalar | 1 |

Missing AVX2 path (8/iteration) -- low priority, normalization is not the bottleneck.

## Industry Comparison

### vs libraw/dcraw

| Feature | libraw/dcraw | This impl | Notes |
|---------|-------------|-----------|-------|
| Black level | Full | Full (via libraw values) | Inherits libraw's computation |
| White balance | Camera/daylight/custom | Camera only | Adequate for astro |
| Bayer demosaic | AHD/VNG/PPG/DCB/DHT/AAHD | **RCD** | 1.6-5.9x faster than libraw |
| X-Trans demosaic | Markesteijn 1/3-pass | Markesteijn 1-pass | 2.1x faster |
| Color matrix | cam_xyz -> sRGB | None | By design (astro linear) |
| Hot pixel removal | Bad pixel map | None | Done at calibration stage |

### vs RawTherapee

| Feature | RawTherapee | This impl |
|---------|------------|-----------|
| Bayer demosaic | AMaZE/RCD/DCB/VNG4/LMMSE/IGV | **RCD** |
| X-Trans demosaic | Markesteijn 1/3-pass | Markesteijn 1-pass |
| Dual demosaic | RCD+VNG4 | None |
| Raw CA correction | Pre-demosaic | None |

### vs darktable

| Feature | darktable | This impl |
|---------|----------|-----------|
| Bayer demosaic | RCD/AMaZE/PPG | **RCD** |
| X-Trans demosaic | Markesteijn 1/3-pass | Markesteijn 1-pass |
| Raw CA correction | Pre-demosaic module | None |
| Hot pixel removal | Hot pixels module | None |

### vs Siril

| Feature | Siril | This impl |
|---------|------|-----------|
| Bayer demosaic | **RCD** (default) | **RCD** |
| X-Trans demosaic | Markesteijn | Markesteijn 1-pass |
| Hot pixel removal | Cosmetic correction | None |

## Missing Features (with Severity)

### Medium

| Feature | Effort | Details |
|---------|--------|---------|
| **Raw CA correction** | Medium (1-2 days) | Pre-demosaic, lateral CA on fast optics |
| **Hot pixel detection** | Low-Medium (1 day) | Median filter on same-color CFA neighbors |

### Low

| Feature | Effort | Details |
|---------|--------|---------|
| AVX2 normalization | Low (hours) | Not bottleneck |
| 3-pass Markesteijn | Medium (1-2 days) | Minimal quality gain for stacked astro |
| Dual demosaic (RCD+VNG4) | Medium (2-3 days) | Better sky background |
| Pre-demosaic noise reduction | High (3-5 days) | Better done post-stack |

### Not Needed (By Design)

Color matrix (cam->sRGB), gamma/tone curve, highlight recovery, output color space.

## Recommendations

1. **Hot pixel pre-demosaic correction** -- simple median filter on same-color neighbors.
2. **Raw CA correction** -- pre-demosaic lateral CA correction for fast optics.

## Issues and Gaps

- ~~`demosaic_bayer()` `todo!()`~~ — **Fixed**: RCD implemented (111ms/24MP, 216 MP/s)
- **Low**: `alloc_uninit_vec` safety (5 call sites, all with SAFETY comments)
- **Low**: Libraw fallback normalizes by 65535.0 (may not use full range)
- **Low**: Output copy in Markesteijn (`arena[4P..7P].to_vec()`, ~70 MB)

## Test Coverage

100+ tests: normalization, black level consolidation, WB multipliers, channel corrections,
FC macro, CFA patterns, BayerImage validation, RCD correctness, XTrans pattern/image/normalization,
Markesteijn steps (green minmax, interpolation, derivatives, homogeneity, SATs, blend),
integration tests, benchmarks. Real-data tests require `--features real-data`.

## Benchmarks

Run: `LUMOS_CALIBRATION_DIR=<path> cargo test -p lumos --release <name> -- --ignored --nocapture`

- `raw_load` -- End-to-end load time
- `bench_load_raw_libraw_demosaic` -- libraw quality levels comparison
- `bench_markesteijn_quality_vs_libraw` -- MAE/PSNR/correlation per channel

Reference (X-Trans 6032x4028): ~1238ms total / ~620ms demosaic, 2.1x faster than libraw.
