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
      rcd.rs          - RCD algorithm (5 steps, rayon row-parallel, ~660 lines)
      tests.rs        - 20+ tests (11 CFA pattern + 9+ RCD correctness)
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
3. **Raw chromatic aberration correction** (optional, pre-demosaic -- darktable/RawTherapee)
4. **White balance** (scale channels by camera WB multipliers)
5. **Demosaic** (interpolate missing color channels)
6. **Color matrix** (cam_xyz or rgb_cam: camera color space -> XYZ -> sRGB)
7. **Gamma / tone curve** (linear -> perceptual encoding)

This implementation covers steps 1, 4, 5. Steps 6-7 are intentionally omitted for the
astrophotography workflow (linear camera-native color space). Color matrix conversion is
not needed for stacking/calibration. Gamma is applied later in the display pipeline.

**Pipeline order is correct**: black subtraction -> WB -> demosaic matches the standard.
Pre-demosaic WB is the accepted best practice per research by Viggiano and as implemented
in dcraw/libraw. The demosaic algorithm sees balanced channel values, producing better
interpolation results. This implementation correctly applies WB before demosaic for both
Bayer and X-Trans paths.

**Astrophotography-specific pipeline correctness**: Siril (the leading open-source astro
processing tool) also uses RCD as its default Bayer demosaic algorithm. The astrophotography
standard is to calibrate (dark/flat/bias subtraction) on raw CFA data BEFORE demosaicing,
which this implementation supports via `load_raw_cfa()`.

### Black Level Consolidation

Replicates libraw's `adjust_bl()` from `utils_libraw.cpp:464-540`:
1. Fold spatial pattern into per-channel `cblack[0..3]` (Bayer 2x2 / X-Trans 1x1)
2. Extract common minimum across channels, move to scalar `black`
3. Handle remaining spatial pattern (rare)
4. Final: `per_channel[c] = cblack[c] + black`, `inv_range = 1/(max - common)`
5. `channel_delta_norm[c] = (per_channel[c] - common) * inv_range`

Two-pass normalization: SIMD pass applies common black, per-pixel pass applies channel delta + WB.

**Assessment**: The consolidation logic is correct and thorough. The cblack array (4104 elements)
stores per-channel corrections in [0..3], spatial block size in [4..5], and per-pixel black
level pattern in [6..6+cblack[4]*cblack[5]]. The implementation handles all three cases:
standard Bayer 2x2 spatial fold, X-Trans 1x1 fold, and remaining spatial patterns. Warning
log for unhandled spatial patterns after consolidation is appropriate.

### White Balance

- `compute_wb_multipliers(cam_mul)`: normalizes so min=1.0 (avoids clipping, same as dcraw -H 0)
- `cam_mul[3]==0` -> copies from `cam_mul[1]` (green) for 3-color cameras
- Returns `None` for invalid -> WB skipped
- CFA/calibration path: no WB applied (correct for calibration frames)
- X-Trans: WB folded into `read_normalized()` per-pixel path

**Assessment**: The min-normalization approach matches dcraw's default behavior. This means
channels with multipliers >1.0 will be scaled up, which can cause clipping in highlights.
This is the standard behavior and is acceptable for astrophotography where well-exposed
sub-frames rarely saturate. The "pink highlights" problem (clipping after WB multiplication)
is a known industry issue that only matters for daytime photography with blown highlights.

### SIMD Normalization

| Architecture | Instruction Set | Elements/Iteration |
|---|---|---|
| x86_64 | SSE4.1 | 4 (128-bit, `_mm_cvtepu16_epi32`) |
| x86_64 | SSE2 | 4 (128-bit, `_mm_unpacklo_epi16`) |
| aarch64 | NEON | 4 (128-bit) |
| Other | Scalar | 1 |

Missing AVX2 path (8/iteration) -- low priority, normalization is not the bottleneck.

## Current State (What's Correct)

### Black Level Processing
- Faithful replication of libraw's `adjust_bl()` for consolidating cblack spatial patterns
- Correct two-pass approach: SIMD uniform black + per-pixel channel delta
- Handles Bayer 2x2, X-Trans 1x1, and rare remaining spatial patterns
- Proper `inv_range` computation from `maximum - common_black`
- Assert on invalid black level (effective_max <= 0)

### White Balance
- Min-normalization matches dcraw standard (prevents channel reduction, accepts possible clipping)
- Correct fallback for 3-color cameras (`cam_mul[3]==0` -> copy from green)
- Proper validation (positive, finite values)
- CFA calibration path correctly skips WB

### Pipeline Order
- Black -> WB -> Demosaic: correct per Viggiano research and dcraw/libraw standard
- Pre-demosaic WB applied to both Bayer and X-Trans paths
- Calibration path (load_raw_cfa) correctly applies black correction without WB

### Bayer Demosaic (RCD)
- Faithful RCD v2.3 implementation (Luis Sanz Rodriguez)
- Correct 5-step pipeline: VH direction, LPF, green interp, R/B interp, border handling
- Ratio-corrected estimation in LPF domain reduces color artifacts
- Rayon row-parallel throughout, buffer reuse (scratch triple-use)
- 1.6-5.9x faster than libraw alternatives, sub-milliarcsecond MAE vs reference
- RCD is the default in Siril (leading astro tool), chosen for star morphology

### X-Trans Demosaic (Markesteijn 1-pass)
- Faithful to dcraw/libraw reference implementation
- Hex lookup, interpolation weights, YPbPr coefficients all match reference
- Homogeneity threshold (8x min) and blend threshold (7/8 max) match reference
- Per-channel normalization + WB correctly integrated in `read_normalized()`
- 2.1x faster than libraw single-threaded, MAE ~0.0005 vs libraw 1-pass
- Memory-optimized arena (10P, never materializes full RGB buffer)

### Sensor Detection
- Correct filters/colors dispatch: monochrome (filters=0 or colors=1), X-Trans (filters=9),
  Bayer (known 2x2 patterns), Unknown (libraw fallback)
- CFA pattern extraction handles both green indices (1 and 3) as green

### RAII and Safety
- LibrawGuard/ProcessedImageGuard properly clean up libraw allocations
- Null-safe drop implementations
- File buffer lifetime managed correctly (dropped before X-Trans demosaic to reduce peak memory)
- alloc_uninit_vec with SAFETY comments at all 5 call sites

### Test Coverage
- 100+ tests across all submodules
- Black level consolidation: uniform, per-channel, Bayer 2x2 fold, X-Trans 1x1 fold
- WB: normal, 3-channel, normalization, all-zeros, negative, NaN
- Channel corrections: identity, delta-only, WB-only, combined, negative clamping
- RCD: all 4 CFA patterns, uniform/gradient/edge inputs, border handling, VH direction sensitivity
- Markesteijn: all 5 steps individually tested, SATs, homogeneity, color interp lookup
- Real-data integration tests behind `real-data` feature flag
- Quality benchmarks against libraw (MAE, PSNR, Pearson correlation)

## Issues Found

### Minor Issues

1. **Libraw fallback normalization by fixed 65535.0**: The `demosaic_libraw_fallback()` method
   normalizes 16-bit output by dividing by 65535.0. This is correct for libraw's 16-bit output
   (which uses the full u16 range after processing), but the comment "Normalize to 0.0-1.0"
   could note that libraw already applies its own black/WB/scaling internally. Not a bug, just
   a documentation gap. (File: `mod.rs:559-563`)

2. **Output copy in Markesteijn**: `arena.storage[4P..7P].to_vec()` allocates ~70 MB for
   the output copy. Could be avoided by writing output directly to a caller-provided buffer
   or by returning the arena and exposing a view. Low priority since it's a single alloc/copy.
   (File: `markesteijn.rs:173`)

3. **README.md is stale**: The README still says "TODO: DCB" for Bayer demosaic in the pipeline
   table, but RCD has been implemented. (File: `README.md:36`)

4. **No clamping after WB in apply_channel_corrections**: The `.min(1.0)` clamp in
   `apply_channel_corrections` may clip highlights that exceed 1.0 after WB multiplication.
   This matches dcraw's default behavior (H=0) but means highlight detail is lost for channels
   with WB multipliers >1.0. For astrophotography sub-frames this is rarely an issue since
   properly exposed subs don't saturate. However, if the user ever processes daytime RAW files,
   this clipping would be visible. Consider documenting this as intentional behavior.
   (File: `mod.rs:259`)

### Non-Issues (Verified Correct)

1. **Pre-demosaic WB order**: WB is applied before demosaic in both Bayer and X-Trans paths.
   This is the correct industry standard per dcraw, libraw, and Viggiano research.

2. **Min-normalization of WB multipliers**: Normalizing so minimum=1.0 is the dcraw default
   and appropriate for astrophotography. The alternative (max=1.0, -H 1+ in dcraw) is only
   useful for highlight recovery in overexposed daytime photos.

3. **X-Trans 4-channel to 3-channel black/WB conversion**: The conversion from libraw's
   4-channel (R, G1, B, G2) to 3-channel (R, G, B) by taking indices [0,1,2] is correct
   because X-Trans has no G2 concept (the 6x6 pattern uses only R=0, G=1, B=2).

4. **alloc_uninit_vec usage**: All 5 call sites have SAFETY comments documenting that every
   element is written before being read. The pattern is correct for avoiding kernel page
   zeroing on large buffers.

## Industry Comparison

### vs libraw/dcraw

| Feature | libraw/dcraw | This impl | Notes |
|---------|-------------|-----------|-------|
| Black level | Full | Full (via libraw values) | Inherits libraw's computation |
| White balance | Camera/daylight/custom | Camera only | Adequate for astro |
| Bayer demosaic | AHD/VNG/PPG/DCB/DHT/AAHD | **RCD** | 1.6-5.9x faster than libraw |
| X-Trans demosaic | Markesteijn 1/3-pass | Markesteijn 1-pass | 2.1x faster |
| Highlight recovery | -H 0/1/2/9 modes | None (clamp to 1.0) | By design (astro workflow) |
| Color matrix | cam_xyz -> sRGB | None | By design (astro linear) |
| Hot pixel removal | Bad pixel map | None | Done at calibration stage |

### vs RawTherapee

| Feature | RawTherapee | This impl |
|---------|------------|-----------|
| Bayer demosaic | AMaZE/RCD/DCB/VNG4/LMMSE/IGV | **RCD** |
| X-Trans demosaic | Markesteijn 1/3-pass | Markesteijn 1-pass |
| Dual demosaic | RCD+VNG4 (detail+flat areas) | None |
| Raw CA correction | Pre-demosaic lateral CA | None |
| Hot pixel removal | Hot/dead pixel filter | None |

### vs darktable

| Feature | darktable | This impl |
|---------|----------|-----------|
| Bayer demosaic | RCD/AMaZE/PPG | **RCD** |
| X-Trans demosaic | Markesteijn 1/3-pass | Markesteijn 1-pass |
| Raw CA correction | Pre-demosaic module (Bayer only) | None |
| Hot pixel removal | Hot pixels module | None |
| Highlight reconstruction | clip/unclip/blend/rebuild modes | None |

### vs Siril (Closest Astro Competitor)

| Feature | Siril | This impl |
|---------|------|-----------|
| Bayer demosaic | **RCD** (default), VNG, bilinear | **RCD** |
| X-Trans demosaic | Markesteijn | Markesteijn 1-pass |
| Calibration before demosaic | Yes (standard workflow) | Yes (via `load_raw_cfa()`) |
| Hot pixel removal | Cosmetic correction | None |
| Color calibration | SPCC (spectrophotometric) | None (by design) |

### vs PixInsight

| Feature | PixInsight | This impl |
|---------|-----------|-----------|
| Bayer demosaic | VNG, AHD, bilinear, SuperPixel | **RCD** |
| Calibration before demosaic | Yes (ImageCalibration module) | Yes (via `load_raw_cfa()`) |
| Hot pixel removal | CosmeticCorrection | None |
| Drizzle integration | Yes | N/A (separate module) |

## Missing Features (with Severity)

### Medium -- POSTPONED

| Feature | Effort | Details |
|---------|--------|---------|
| **Raw CA correction** | Medium (1-2 days) | Pre-demosaic lateral CA, Bayer only. darktable/RawTherapee both implement this. Relevant for fast optics (f/2.8 and below) where lateral CA is visible even after stacking. |
| **Hot pixel detection** | Low-Medium (1 day) | Median filter on same-color CFA neighbors in raw CFA data. Both Siril and PixInsight do this before demosaic. Currently handled at calibration stage but no pre-demosaic detection exists. |

### Low -- POSTPONED

| Feature | Effort | Details |
|---------|--------|---------|
| AVX2 normalization | Low (hours) | Not bottleneck. Would process 8 elements/iteration vs current 4. |
| 3-pass Markesteijn | Medium (1-2 days) | Minimal quality gain for stacked astro per community reports. 1-pass vs 3-pass quality delta (MAE ~0.0003) is negligible after frame stacking. |
| Dual demosaic (RCD+VNG4) | Medium (2-3 days) | Better sky background in flat regions. RawTherapee feature. |
| Pre-demosaic noise reduction | High (3-5 days) | Better done post-stack in astro workflow. |
| Highlight recovery | Low (1 day) | Only needed for daytime photos. Astro sub-frames should not clip. |

### Not Needed (By Design)

- **Color matrix** (cam->sRGB): Astrophotography works in linear camera-native color space.
  Color calibration happens post-stack (photometric color calibration against star catalogs).
- **Gamma/tone curve**: Applied later in the display pipeline, not during raw loading.
- **Output color space conversion**: Linear data preserved for stacking/calibration math.
- **Highlight recovery**: Astrophotography sub-frames should be exposed below saturation.
  If highlights clip, the data is genuinely lost (unlike daytime photography where partial
  channel information can be reconstructed).

## Recommendations -- POSTPONED

1. **Hot pixel pre-demosaic correction** -- simple median filter on same-color CFA neighbors.
   This is the single most impactful missing feature for astrophotography quality. Hot pixels
   in raw data produce colored crosses after demosaic that are hard to remove post-facto.

2. **Raw CA correction** -- pre-demosaic lateral CA correction for fast optics. Only affects
   Bayer sensors (darktable/RawTherapee note X-Trans doesn't benefit). Relevant for users
   with fast refractors or camera lenses.

3. **Fix README.md** -- Update pipeline table to show RCD instead of "TODO: DCB".

## Detailed Design Notes

### WB Clipping Behavior

The `apply_channel_corrections` function clamps output to [0.0, 1.0] after applying
`(pixel - delta).max(0.0) * wb_mul`. This means for a typical camera where red WB
multiplier is ~2.0, pixels at 60% of the raw range will be clipped to 1.0 in the red
channel. This matches dcraw's default -H 0 behavior and is the standard approach. The
alternative (-H 1+, normalize so max_mul=1.0) prevents clipping but darkens the image.
For astrophotography, the -H 0 behavior is correct because:
- Sub-frames should not have clipped highlights
- The WB multipliers ensure color channels have equal response to white light
- Stacking operates on properly white-balanced, clipped data

### X-Trans Memory Optimization

The X-Trans path has a memory-saving trick: raw u16 data is copied to a Vec<u16> (47 MB
for 24 MP) and the libraw guard + file buffer are dropped before demosaicing starts. This
reduces peak memory by ~77 MB compared to keeping libraw alive during demosaic. The per-pixel
normalization happens on-the-fly in `read_normalized()` during the Markesteijn algorithm,
avoiding a separate P*4-byte f32 normalization buffer.

### Two-Pass vs Single-Pass Normalization

For Bayer sensors, normalization uses two passes:
1. SIMD pass: subtract common black, scale by inv_range (uniform across all channels)
2. Per-pixel pass: subtract per-channel delta, multiply by WB (channel-dependent)

This split is an optimization: the first pass is fully vectorizable (no branching on channel),
while the second pass handles the channel-dependent corrections. For cameras with uniform
black levels across channels (delta_norm all zeros) and no WB, the second pass is skipped
entirely.

For X-Trans sensors, there is only a single-pass per-pixel normalization in `read_normalized()`
because the 6x6 pattern makes SIMD vectorization of the first pass impractical.

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
- `bench_bayer_rcd_demosaic` -- RCD timing vs libraw PPG/AHD/DHT
- `bench_bayer_rcd_quality_vs_libraw` -- RCD quality vs libraw AHD/PPG/DHT
- `bench_rcd_demosaic_core` -- Synthetic data, isolates demosaic performance

Reference (X-Trans 6032x4028): ~1238ms total / ~620ms demosaic, 2.1x faster than libraw.
Reference (Bayer 8736x5856): ~954ms RCD vs 1559ms PPG / 2731ms AHD / 5627ms DHT.
