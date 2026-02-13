# calibration_masters Module

## Overview

Creates master calibration frames (dark, flat, bias) from raw CFA sensor data and
applies them to calibrate light frames. Detects defective pixels (hot and cold/dead)
via MAD-based statistics. Operates on raw single-channel CFA data before demosaicing.

**Files:** mod.rs (orchestration), defect_map.rs (detection/correction), tests.rs (integration tests)

## Architecture

```
CalibrationMasters::from_raw_files()     CalibrationMasters::new()
  stack_cfa_frames(darks, ...)             Takes pre-built CfaImages
  stack_cfa_frames(flats, ...)             Generates DefectMap from dark
  stack_cfa_frames(biases, ...)
  stack_cfa_frames(flat_darks, ...)
         |                                          |
         v                                          v
CalibrationMasters { master_dark, master_flat, master_bias, master_flat_dark, defect_map }
         |
         v
calibrate(light):
  1. Dark subtraction (or bias-only fallback)     CfaImage::subtract()
  2. Flat division (flat dark > bias for flat norm) CfaImage::divide_by_normalized()
  3. CFA-aware defective pixel correction            DefectMap::correct()
```

## Calibration Formula

**Implementation:** `calibrated = (Light - Dark) / normalize(Flat - FlatSub)`

Where `normalize(X) = X / mean(X)`, and `FlatSub` is the flat dark if provided,
otherwise bias. Flat dark takes priority because it matches the flat's exposure time
and captures both bias and dark current accumulated during the flat exposure.

**Industry standard (Siril, PixInsight, Astropy CCD Guide):**
`L_c = (L - D) / (F - F_d)` with normalization by `mean(F - F_d)`.
Where `F_d` is a dark frame matching the flat's exposure time. When no flat dark
is available, bias is used as fallback.

**Verdict:** The formula is correct. The implementation matches the standard. When both
dark and bias exist, dark is subtracted from the light (dark already contains bias),
and flat dark (or bias fallback) is subtracted from the flat before normalization.

The flat division guards against divide-by-zero with `norm_flat > f32::EPSILON` and
uses `f64` accumulation for the mean, which is good numerical practice.

## Comparison with Industry Standards

### Calibration Order

| Step | This implementation | Siril | PixInsight | Astropy CCD Guide |
|------|-------------------|-------|------------|-------------------|
| 1 | Dark subtraction (or bias fallback) | Dark subtraction (with optional optimization) | Dark subtraction (with optional scaling) | Overscan/bias, then dark subtraction |
| 2 | Flat division (flat dark > bias for flat norm) | Flat division (bias subtracted from flat) | Flat division (separate CFA scaling optional) | Flat division |
| 3 | CFA-aware defect correction | Cosmetic correction (separate step) | CosmeticCorrection (separate process) | Bad pixel masking |

**Verdict:** The order is correct and matches the standard pipeline. The industry-standard
sequence is: (1) subtract dark/bias from light, (2) divide by normalized flat, (3) cosmetic
correction. This implementation follows that sequence exactly.

One subtle correctness point: dark subtraction before flat division is essential because
the flat field only describes sensitivity variations. Dark current is additive and independent
of pixel sensitivity, so it must be removed first. This implementation gets this right.

### CFA-Aware Processing

**This implementation:** All calibration operates on raw single-channel CFA data before
demosaicing. Dark subtraction, flat division, and defect correction all work on the raw
Bayer/X-Trans mosaic. Defect correction uses same-color CFA neighbors (stride-2 for
Bayer, radius-6 for X-Trans).

**Industry standard:** All professional tools (PixInsight, Siril, APP) perform calibration
on raw CFA data before demosaicing. Siril explicitly documents: "perform image calibration
and cosmetic correction with raw CFA data, then debayer."

**PixInsight CFA flat scaling:** PixInsight offers "Separate CFA flat scaling factors" which
computes 3 independent normalization factors (one per R, G, B CFA channel) to prevent
color shifts from non-white flat illumination. This implementation uses a single global
mean for flat normalization.

**Siril equalize_cfa:** Siril offers `equalize_cfa` / `grey_flat` which equalizes the
median intensities of RGB layers in the master flat to prevent color tinting.

**Verdict:** CFA-aware operation before demosaic is correct and matches all professional
tools. However, the single global mean for flat normalization could cause color shifts
if the flat light source is not perfectly white (which is common with LED panels, twilight
flats, etc.). Per-CFA-channel normalization would be more robust. See Issues section.

### Dark Frame Handling

**This implementation:** Darks are stored raw (bias + thermal). No dark scaling by exposure
time or temperature. Darks must match light exposure time and temperature.

**PixInsight:** Supports dark frame optimization (scales dark thermal component to minimize
noise in calibrated output). Also supports exposure-time-based scaling (`dark_scaled =
bias + (dark - bias) * t_light / t_dark`). Requires bias-subtracted dark for proper
scaling. Dark optimization iteratively finds optimal scaling factor k.

**Siril:** Supports dark optimization with `-opt` flag. Can scale by exposure keyword with
`-opt=exp`. Automatically calculates coefficient to apply to dark.

**Astropy CCD Guide:** Documents dark current scaling linearly with exposure time for CCDs:
`dark_current = gain * dark_counts / exposure_time`.

**CMOS reality:** Dark current scaling is unreliable for CMOS sensors due to amp glow (does
not scale linearly with time), non-linear dark current in some pixels, and sense-node dark
current behaving differently from photodiode dark current. Modern CMOS best practice is to
use matched darks at the same exposure time and temperature.

**Verdict:** Not supporting dark scaling is an acceptable design choice for a CMOS-focused
workflow. The metadata fields (`exposure_time`, `ccd_temp`, `set_temp`) are already present
in `AstroImageMetadata`, so scaling could be added later for CCD users if needed. The
README.md correctly documents this as a current limitation.

### Negative Value Handling

**This implementation:** `CfaImage::subtract()` does NOT clamp to zero. The f32 pipeline
preserves negative values, which is documented in the code: "the f32 pipeline preserves
negatives, and stacking averages them out correctly. Clamping to zero would introduce a
positive bias in the stacked result."

**PixInsight:** Preserves negative values during calibration as intermediate results, avoiding
clipping. Offers an optional output pedestal to prevent negatives in final output.

**Standard practice:** Preserving negative values after dark subtraction is correct and
important for photometric accuracy. Clamping to zero introduces a systematic positive bias
because noise below zero is lost.

**Verdict:** Correct. Preserving negatives is the right approach and matches PixInsight
behavior.

### Flat Field Normalization

**This implementation:** `divide_by_normalized()` computes `mean(flat - bias)` over ALL
pixels (global mean), then divides: `light /= (flat[i] - bias[i]) / mean(flat - bias)`.
This preserves the overall flux level of the light frame.

**PixInsight:** Offers either global mean or per-CFA-channel means. The "Separate CFA flat
scaling factors" option computes 3 CFA scaling factors (R, G, B channels) independently.

**Siril:** Uses mean of the master flat calibrated with master bias. Offers `equalize_cfa`
to equalize per-channel medians.

**Astropy:** Normalizes by the mean of the flat (usually the central region, but global
mean is also common).

**Verdict:** Using global mean is the simplest correct approach. It works well when the
flat light source has neutral color. Per-CFA-channel normalization is more robust and
prevents color tinting from non-white flat sources, but requires more complex implementation.

### Defect Map Detection

**This implementation:**
- MAD-based sigma estimation (robust to outliers)
- Per-CFA-color statistics (R, G, B tested independently)
- Default 5.0 sigma threshold
- Sigma floor: `max(computed_sigma, median * 0.1, 1e-4)`
- Detects both hot (above upper) and cold (below lower) from master dark

**PixInsight CosmeticCorrection:**
- Uses master dark or auto-detect mode
- 3 sigma default threshold
- Local window statistics (not global)
- Replaces with average of surrounding pixels

**Siril Cosmetic Correction:**
- Uses average deviation (avgDev), not MAD
- Hot: `pixel > m_5x5 + max(avgDev, sigma_high * avgDev)`
- Cold: `pixel < m_5x5 - sigma_low * avgDev`
- Hot replaced by 3x3 average (with validation check)
- Cold replaced by 5x5 median
- Global statistics, not per-CFA-channel

**Astropy CCD Guide:**
- Direct dark current threshold (e.g., > 4 e-/sec)
- Compares dark current rates across different exposure times
- No statistical sigma method

**AstroPixelProcessor (APP):**
- Cold pixels from flats: `pixel < cold_pct * median(flat)` (typically 50%)
- Hot pixels from darks: sigma-based detection
- Separate maps combined

**Verdict:**
- MAD is statistically superior to avgDev (Siril) and standard deviation for outlier
  detection because it is robust to the very outliers being detected (breakdown point of 50%).
- Per-CFA-color statistics is better than Siril's global approach and on par with PixInsight.
- The 5-sigma default is conservative compared to PixInsight's 3-sigma default. This reduces
  false positives but may miss marginal hot pixels. Could offer user control.
- One gap: cold pixel detection from master dark alone is less reliable than from flats
  (see Issues). Dead pixels show zero response in flats, which is more definitive than
  low dark current.

### Defect Correction Method

**This implementation:** Median of same-color CFA neighbors for both hot and cold pixels.
- Bayer: 8 same-color neighbors at stride 2
- X-Trans: up to 24 same-color neighbors within radius 6, sorted by Manhattan distance
- Mono: 8-connected neighbor median

**Industry comparison:**
- PixInsight: Average of surrounding pixels
- Siril: Average for hot (3x3), median for cold (5x5)
- Scientific standard: Median is preferred for robustness against nearby defects

**Verdict:** Median replacement is the most robust choice. CFA-aware correction at stride-2
for Bayer is correct and essential for preserving the mosaic pattern before demosaicing.
This is better than tools that perform cosmetic correction after demosaicing.

## Issues

### ~~Medium: Sigma Floor Fails When Median is Zero~~ -- FIXED
- Added absolute floor: `sigma = computed_sigma.max(median * 0.1).max(1e-4)`
- `1e-4` in [0,1] range is about 6.5 ADU in 16-bit, prevents over-detection while still
  catching genuine hot pixels. Test: `test_defect_detection_zero_median_no_false_positives`

### Medium: No Per-CFA-Channel Flat Normalization
- **File:** cfa.rs `divide_by_normalized()` uses a single global `mean(flat - bias)` to
  normalize the flat field.
- PixInsight offers "Separate CFA flat scaling factors" (3 independent R/G/B means).
  Siril offers `equalize_cfa` / `grey_flat` for per-channel equalization.
- When the flat light source is not perfectly white (LED panels, twilight flats, T-shirt
  flats), the R/G/B channels in the flat have different mean values. A single global
  mean then introduces a color shift: bright channels are over-corrected, dim channels
  under-corrected. The result is a color tint in the calibrated image.
- **Fix:** Compute per-CFA-color means (mean of R pixels, mean of G pixels, mean of B
  pixels separately), then normalize each pixel by its own color's mean. For Bayer, this
  means 3 normalization factors. For X-Trans, also 3 (R/G/B).
- **Impact:** Moderate. Only affects OSC (one-shot color) cameras with non-neutral flat
  sources. Monochrome cameras are unaffected.

### Medium: Cold Pixel Detection Only from Master Dark
- Cold/dead pixels are detected by looking for low values in the master dark. However,
  dead pixels (zero or near-zero sensitivity) are more reliably detected from master flats,
  where they appear as pixels with abnormally low response to uniform illumination.
- APP uses separate maps: hot pixels from darks, cold pixels from flats.
- A pixel with zero dark current is not necessarily dead; it could just be a very clean
  pixel. A pixel with zero flat response IS dead.
- **Fix:** Add optional cold pixel detection from master flat: flag pixels where
  `flat_pixel < threshold * median(flat)` (APP uses 50% as default threshold).
- **Impact:** May miss some dead pixels that happen to have normal dark current but
  zero photon sensitivity.

### Low: collect_color_samples Allocates Full Channel Before Subsampling
- **File:** defect_map.rs:229-241
- For CFA mode, collects ALL pixels of target color into a Vec, then subsamples.
  On a 6000x4000 image, this allocates ~6M f32s (~24MB) per color channel just to
  keep 100K samples.
- The mono path already uses strided sampling directly (line 225).
- **Fix:** Use strided iteration for CFA too -- count matching pixels first, compute
  stride, then collect every Nth matching pixel directly.

### Low: from_raw_files Stacks Sequentially
- **File:** mod.rs:121-124
- Darks, flats, biases, and flat_darks are stacked one after another.
- These are completely independent operations.
- **Fix:** Use `rayon::join` or `std::thread::scope` to stack in parallel.
  Would give ~2-4x speedup during master creation with 4 frame types.

### Low: DefectMap::correct is Sequential
- **File:** defect_map.rs:136
- Iterates hot and cold indices sequentially.
- Safe to parallelize: hot pixels are sparse, Bayer stride-2 neighborhoods don't
  overlap. For X-Trans, radius-6 neighborhoods could theoretically overlap but
  hot pixels are rare enough in practice.
- **Fix:** Use `par_iter` with index-based writes or partition by spatial locality.

### Low: No Dark Frame Scaling
- No support for scaling dark frames to different exposure times or temperatures.
- Dark current scales linearly with exposure for CCDs: `dark_current = rate * time`.
- PixInsight formula: `dark_scaled = bias + (dark - bias) * t_light / t_dark`.
  This requires separating bias from thermal component.
- For CMOS sensors, scaling is unreliable (amp glow, non-linear dark current,
  sense-node vs photodiode dark current differences).
- Modern practice: Use matched darks (same exposure/temp) rather than scaling.
- **Status:** Acceptable limitation for CMOS workflows. The metadata fields
  (`exposure_time`, `ccd_temp`) are already available if scaling is needed later.

## Missing Features (Not Implemented, May Not Be Needed)

### Overscan Subtraction
- Professional CCD calibration subtracts the overscan region (virtual pixels read
  after each row) to track per-frame bias drift. Not relevant for CMOS cameras or
  DSLR raw files, which have no overscan region.
- **Status:** Not needed for target use case (CMOS astro cameras, DSLRs).

### Output Pedestal
- PixInsight can add a small positive pedestal to calibrated output to prevent negative
  values from being clipped when saving to unsigned integer formats.
- This implementation uses f32 throughout, so negatives are preserved naturally.
- **Status:** Not needed while pipeline uses f32 internally. Only needed if exporting
  to 16-bit unsigned FITS/TIFF.

### Dark Frame Temperature Scaling
- For cooled CCD cameras, dark current approximately doubles every ~6 degrees C
  (Arrhenius relation). PixInsight's dark optimization can account for temperature
  differences between dark library and light frames.
- For CMOS, temperature scaling is less reliable.
- **Status:** Could be added for CCD support using: `scale = 2^((T_light - T_dark) / 6.3)`

### Bad Pixel Map from Multiple Sources
- APP combines separate bad pixel maps: hot from darks, cold from flats, columns from
  either. The combined map is applied to both lights and flats.
- This implementation only derives defects from the master dark.
- **Status:** Adding flat-based cold pixel detection would improve dead pixel coverage.

### Cosmetic Correction for Bad Columns/Rows
- Some sensors have entire bad columns or rows. Professional tools detect and
  interpolate these separately from individual pixel defects.
- **Status:** Not implemented. Low priority unless users report column defects.

## Adaptive Sampling

For images >200K pixels, statistics are computed on 100K uniformly sampled pixels.
This gives <0.5% median error with >99% confidence per CLT for order statistics.
Avoids O(n log n) sort on full-resolution images.

## Stacking Strategy

- < 8 frames: median (robust to outliers with few frames)
- >= 8 frames: Winsorized sigma-clipped mean at 3.0 sigma (darks/biases),
  sigma-clip at 2.5 sigma (flats)
- Darks/biases: no normalization; Flats: multiplicative normalization

## Test Coverage

- Unit tests for hot pixel detection (small/large images, edge cases, no defects)
- Unit tests for cold/dead pixel detection and mixed hot+cold detection
- Unit tests for correction (Bayer stride-2, Mono 8-connected, corner pixels, cold pixels)
- Per-CFA-color detection tests (hot red in mixed-value Bayer, cold blue)
- Integration tests for full calibration pipeline (dark sub, bias-only, flat correction,
  combined dark+flat+bias with algebraic verification, flat dark priority over bias)
- Dimension mismatch assertions tested with `#[should_panic]`
- Full pipeline test verifies vignetting cancellation algebraically
- Zero-median sigma floor edge case tested (`test_defect_detection_zero_median_no_false_positives`)

## Key Constants and Thresholds

| Constant | Value | Source |
|----------|-------|--------|
| DEFAULT_HOT_PIXEL_SIGMA | 5.0 | Conservative; industry range is 3-5 |
| MAD_TO_SIGMA | 1.4826 | 1/Phi^-1(0.75), standard for normal distributions |
| MAX_MEDIAN_SAMPLES | 100,000 | Adaptive sampling for images >200K pixels |
| Sigma floor (relative) | median * 0.1 | Prevents MAD=0 over-detection on uniform darks |
| Sigma floor (absolute) | 1e-4 | Prevents over-detection when median is also zero |
| Stacking threshold | 8 frames | Below: median; above: sigma-clipped mean |

## References

- Siril calibration: https://siril.readthedocs.io/en/latest/preprocessing/calibration.html
- Siril cosmetic correction: https://siril.readthedocs.io/en/latest/processing/cc.html
- Astropy CCD Reduction Guide: https://www.astropy.org/ccd-reduction-and-photometry-guide/
- PixInsight Master Frames: https://www.pixinsight.com/tutorials/master-frames/
- PixInsight CosmeticCorrection: https://chaoticnebula.com/cosmetic-correction/
- PixInsight dark optimization algorithm: https://pixinsight.com/forum/index.php?threads/dark-frame-optimization-algorithm.8529/
- PixInsight CFA flat scaling: https://pixinsight.com/forum/index.php?threads/when-to-check-enable-cfa-separate-cfa-flat-and-dslr-cmos-calibration-needs.16397/
- Dark current scaling: https://telescope.live/blog/dark-calibration-frame-scaling
- CMOS calibration (no separate bias): https://www.aavso.org/bias-frames-and-cmos-cameras-scaled-and-unscaled-darks
- CMOS calibration workflow: https://astrobasics.de/en/basics/bias-flats-darks-darkflats/
- APP bad pixel maps: https://www.astropixelprocessor.com/community/tutorials-workflows/creating-a-bad-pixel-map/
- Dark frame negative values: https://www.cloudynights.com/topic/638193-dark-subtraction-do-not-cut-off-negative-values/
- Flat-field correction (Wikipedia): https://en.wikipedia.org/wiki/Flat-field_correction
- Dark current in CMOS: https://pdxscholar.library.pdx.edu/cgi/viewcontent.cgi?article=1145&context=phy_fac
