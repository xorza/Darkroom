# calibration_masters Module

## Module Overview

Creates master calibration frames (dark, flat, bias, flat-dark) from raw CFA sensor data and
applies them to calibrate light frames. Detects defective pixels (hot and cold/dead) via
MAD-based statistics. Operates on raw single-channel CFA data before demosaicing.

**Files:** `mod.rs` (orchestration), `defect_map.rs` (detection/correction), `tests.rs` (integration tests)

### Architecture

```
CalibrationMasters::from_raw_files()     CalibrationMasters::new()
  -> Result<Self, stacking::Error>           Takes pre-built CfaImages
  stack_cfa_frames(darks, ...)               Generates DefectMap from dark
  stack_cfa_frames(flats, ...)
  stack_cfa_frames(biases, ...)
  stack_cfa_frames(flat_darks, ...)
         |                                          |
         v                                          v
CalibrationMasters { master_dark, master_flat, master_bias, master_flat_dark, defect_map }
         |
         v
calibrate(light):
  1. Dark subtraction (or bias-only fallback)       CfaImage::subtract()
  2. Flat division (flat dark > bias for flat norm)  CfaImage::divide_by_normalized()
  3. CFA-aware defective pixel correction            DefectMap::correct()
```

### Calibration Formula

`calibrated = (Light - Dark) / normalize(Flat - FlatSub)`

Where `normalize(X) = X / per_channel_mean(X)`, and `FlatSub` is the flat dark if provided,
otherwise bias. Flat dark takes priority because it matches the flat's exposure time and
captures both bias and dark current accumulated during the flat exposure.

For Mono images, a single global mean is used. For Bayer/X-Trans, per-CFA-channel means
are computed (3 independent R/G/B normalization factors) to avoid color shifts from
non-white flat sources. This matches PixInsight's "Separate CFA flat scaling factors"
and Siril's `equalize_cfa`.

### Stacking Strategy

- < 8 frames: median (robust to outliers with few frames)
- >= 8 frames: Winsorized sigma-clipped mean at 3.0 sigma (darks/biases),
  sigma-clip at 2.5 sigma (flats)
- Darks/biases: no normalization; Flats: multiplicative normalization

### Key Constants and Thresholds

| Constant | Value | Justification |
|----------|-------|---------------|
| DEFAULT_HOT_PIXEL_SIGMA | 5.0 | Conservative; industry range is 3-5 |
| MAD_TO_SIGMA | 1.4826 | 1/Phi^-1(0.75), standard for normal distributions |
| MAX_MEDIAN_SAMPLES | 100,000 | Adaptive sampling for images >200K pixels |
| Sigma floor (relative) | median * 0.1 | Prevents MAD=0 over-detection on uniform darks |
| Sigma floor (absolute) | 1e-4 | Prevents over-detection when median is also zero |
| Stacking threshold | 8 frames | Below: median; above: sigma-clipped mean |

---

## Industry Standard Comparison

### Calibration Formula and Order

| Step | This implementation | Siril | PixInsight | Astropy/ccdproc |
|------|-------------------|-------|------------|-----------------|
| 1 | Dark subtraction (or bias fallback) | Dark subtraction (with optional optimization) | Dark subtraction (with optional scaling) | Overscan/bias, then dark subtraction |
| 2 | Flat division (per-CFA-channel means) | Flat division (with equalize_cfa option) | Flat division (separate CFA scaling optional) | Flat division |
| 3 | CFA-aware defect correction | Cosmetic correction (separate step) | CosmeticCorrection (separate process) | Bad pixel masking |

**Verdict: Correct.** The implementation follows the industry-standard sequence exactly.
Dark subtraction before flat division is essential because dark current is additive and
independent of pixel sensitivity. The formula `L_c = (L - D) / normalize(F - F_d)` matches
the standard used by Siril, PixInsight, and Astropy CCD Guide.

When both dark and bias exist, dark is subtracted from the light (dark already contains bias),
and flat dark (or bias fallback) is subtracted from the flat before normalization. The flat
division guards against divide-by-zero with `norm_flat > f32::EPSILON` and uses `f64`
accumulation for the mean, which is good numerical practice.

### CFA-Aware Processing

**This implementation:** All calibration operates on raw single-channel CFA data before
demosaicing. Dark subtraction, flat division, and defect correction all work on the raw
Bayer/X-Trans mosaic.

**Industry standard:** All professional tools (PixInsight, Siril, APP) perform calibration
on raw CFA data before demosaicing. Siril explicitly documents: "perform image calibration
and cosmetic correction with raw CFA data, then debayer."

**Verdict: Correct.** CFA-aware operation before demosaic matches all professional tools.

### Flat Field Normalization

**This implementation:** `divide_by_normalized()` dispatches to:
- **Mono/unknown CFA:** Single global `mean(flat - bias)` normalization.
- **Bayer/X-Trans:** Per-CFA-channel means -- 3 independent R/G/B normalization factors.
  Each pixel is divided by `(flat[i] - bias[i]) / mean_of_same_color_channel`.

**PixInsight:** "Separate CFA flat scaling factors" (since v1.8.8-6) computes 3 independent
CFA channel scaling factors. This avoids color shifts when flat light source is not white.

**Siril:** `equalize_cfa` / `grey_flat` equalizes mean intensity of RGB layers in the
master flat to prevent color tinting.

**Verdict: Correct and complete.** Per-CFA-channel normalization matches PixInsight's
approach and addresses the same problem as Siril's equalize_cfa.

### Negative Value Handling

**This implementation:** `CfaImage::subtract()` does NOT clamp to zero. The f32 pipeline
preserves negative values. Documented: "Clamping to zero would introduce a positive bias."

**PixInsight:** Preserves negative values during calibration. Optional output pedestal to
prevent negatives in final output.

**Standard practice:** Preserving negatives after dark subtraction is correct and important
for photometric accuracy. Clamping introduces systematic positive bias.

**Verdict: Correct.** Matches PixInsight behavior and scientific best practice.

### Dark Frame Handling

**This implementation:** Darks stored raw (bias + thermal). No dark scaling by exposure
time or temperature. Darks must match light exposure time and temperature.

**PixInsight:** Dark frame optimization: purely numeric scaling of thermal component to
minimize residual noise in calibrated output. Also supports exposure-time-based scaling:
`dark_scaled = bias + (dark - bias) * t_light / t_dark`. Requires bias-subtracted dark.

**Siril:** Dark optimization with `-opt` flag. Coefficient scaling. `-opt=exp` uses FITS
exposure keywords.

**CMOS reality:** Dark scaling is unreliable for CMOS sensors due to amp glow (non-linear
with time), non-linear dark current, and different behavior of sense-node vs photodiode dark
current. Modern best practice: use matched darks (same exposure/temperature).

**Verdict: Acceptable.** Not supporting dark scaling is a deliberate, correct design choice
for CMOS-focused workflows. The metadata fields (`exposure_time`, `ccd_temp`, `set_temp`)
are already present if scaling is needed later for CCD users.

### Defect Map Detection Algorithm

**This implementation (defect_map.rs):**
- MAD-based sigma estimation (robust to outliers, breakdown point 50%)
- Per-CFA-color statistics (R, G, B tested independently)
- Default 5.0 sigma threshold
- Sigma floor: `max(computed_sigma, median * 0.1, 1e-4)`
- Detects both hot (above upper) and cold (below lower) from master dark
- Adaptive sampling: 100K samples per channel for images >200K pixels

**PixInsight CosmeticCorrection:**
- Three modes: auto-detect (local window), master dark, defect list
- Auto-detect uses local window statistics with sigma-based thresholds
- 3 sigma default threshold
- Replaces with average of surrounding pixels

**Siril Cosmetic Correction:**
- Uses average deviation (avgDev), not MAD
- Hot: `pixel > m_5x5 + max(avgDev, sigma_high * avgDev)`
- Cold: `pixel < m_5x5 - sigma_low * avgDev`
- Hot replaced by 3x3 average (with validation check)
- Cold replaced by 5x5 median
- Global statistics, not per-CFA-channel
- CFA mode uses stride-2 for Bayer, but does NOT support X-Trans

**AstroPixelProcessor (APP):**
- Cold pixels from flats: `pixel < cold_pct * median(flat)` (typically 50%)
- Hot pixels from darks: kappa-sigma detection (MRS noise estimate)
- Separate maps combined from multiple sources

**Astropy CCD Guide:**
- Direct dark current threshold (e.g., > 4 e-/sec)
- Compares dark current rates across different exposure times

**Verdict:**
- MAD is statistically superior to avgDev (Siril) for outlier detection. MAD has a
  breakdown point of 50% vs avgDev's sensitivity to the outliers being detected.
- Per-CFA-color statistics is better than Siril's global approach and matches PixInsight.
- The 5-sigma default is conservative compared to PixInsight's 3-sigma and APP's 2-3 kappa.
  This reduces false positives but may miss marginal hot pixels.
- X-Trans support (radius-6 same-color search) exceeds Siril which doesn't support X-Trans.

### Defect Correction Method

**This implementation:** Median of same-color CFA neighbors for both hot and cold pixels.
- Bayer: 8 same-color neighbors at stride 2
- X-Trans: up to 24 same-color neighbors within radius 6, sorted by Manhattan distance
- Mono: 8-connected neighbor median

**Industry comparison:**
- PixInsight: Average of surrounding pixels
- Siril: Average for hot (3x3), median for cold (5x5)
- Scientific standard: Median preferred for robustness against clusters of nearby defects

**Verdict: Correct and robust.** Median replacement is the best choice. CFA-aware correction
at stride-2 for Bayer is correct and essential for preserving the mosaic pattern before
demosaicing. X-Trans support is a differentiator. This is better than tools that perform
cosmetic correction after demosaicing.

---

## Missing Features (with severity)

### High: No User-Configurable Sigma Threshold -- POSTPONED

The default 5.0-sigma threshold is hardcoded in `DEFAULT_HOT_PIXEL_SIGMA`. PixInsight
defaults to 3 sigma, APP uses 2-3 kappa. Different sensors and conditions need different
thresholds (clean cooled CCD vs warm CMOS with many hot pixels).

**Impact:** Users cannot tune detection sensitivity. A sigma parameter on `calibrate()` or
`CalibrationMasters::new()` would be trivial to add.

### Medium: Cold Pixel Detection Only from Master Dark -- POSTPONED

Cold/dead pixels are detected from low values in the master dark. However, dead pixels
(zero sensitivity) are more reliably detected from master flats, where they appear as
pixels with abnormally low response to uniform illumination.

- APP uses separate maps: hot from darks, cold from flats.
- A pixel with low dark current is not necessarily dead (could be a clean pixel).
  A pixel with zero flat response IS dead.
- **Fix:** Add optional cold pixel detection from master flat: flag pixels where
  `flat_pixel < threshold * median(flat)` (APP uses 50% as default).
- **Impact:** May miss dead pixels that have normal dark current but zero photon sensitivity.

### Medium: No Defect Correction Applied to Master Flat -- POSTPONED

The defect map is applied only to light frames during `calibrate()`. Hot pixels in the
master flat itself are not corrected, which means the flat division at hot pixel locations
uses a corrupted flat value before the defect correction step replaces it.

- Industry practice: APP applies the bad pixel map to both lights and flats.
- PixInsight's workflow: CosmeticCorrection is a separate step applied after calibration,
  but master flats are calibrated with bias/dark which removes additive hot pixel signal.
- **Impact:** Low in practice. Hot pixels in the flat are mostly from thermal noise (already
  removed by flat dark/bias subtraction during normalization). Remaining sensitivity-based
  defects are small. Still, applying defect correction to the master flat after stacking
  would be more rigorous.

### Low: No Dark Frame Scaling -- POSTPONED

No support for scaling dark frames to different exposure times or temperatures.

- PixInsight formula: `dark_scaled = bias + (dark - bias) * t_light / t_dark`
- Siril: dark optimization coefficient, `-opt=exp` for FITS keywords
- For CMOS: scaling is unreliable (amp glow, non-linear dark current).
- Modern practice: use matched darks (same exposure/temp).
- **Status:** Acceptable for CMOS workflows. Metadata fields already exist for future CCD support.

### Low: No Overscan Subtraction -- POSTPONED

Professional CCD calibration subtracts the overscan region to track per-frame bias drift.
Not relevant for CMOS cameras or DSLR raw files.

- **Status:** Not needed for target use case (CMOS astro cameras, DSLRs).

### Low: No Output Pedestal -- POSTPONED

PixInsight can add a positive pedestal to prevent negative values when saving to unsigned
integer formats. This implementation uses f32 throughout.

- **Status:** Not needed while pipeline uses f32 internally.

### Low: No Bad Column/Row Detection -- POSTPONED

Some sensors have entire bad columns or rows. Professional tools detect and interpolate
these separately.

- PixInsight: LinearDefectDetection + LinearPatternSubtraction
- **Status:** Low priority unless users report column defects.

### Low: No Auto-Detect Mode for Cosmetic Correction -- POSTPONED

PixInsight CosmeticCorrection offers "auto-detect" which uses local window statistics on
each light frame (not just the master dark) to find defects. This catches intermittent
defects or pixels that become hot during acquisition.

- **Status:** The master dark approach catches persistent defects. Auto-detect on individual
  frames before stacking could catch intermittent ones, but sigma-clipped stacking already
  rejects transient outliers.

---

## Correctness Issues

### None Found in Core Calibration

The calibration formula, order of operations, and negative value handling are all correct
and match industry standards. Specific verifications:

1. **Dark subtraction removes both bias and thermal** -- correct. The dark frame contains
   `bias + thermal`, so `Light - Dark = signal * vignetting`.
2. **Flat normalization with bias/flat-dark subtraction** -- correct. `normalize(Flat - Bias)`
   produces a pure sensitivity profile.
3. **Per-CFA-channel flat normalization** -- correct. Prevents color shifts from non-white
   flat sources.
4. **Flat dark takes priority over bias for flat normalization** -- correct. Flat dark
   captures both bias and dark current at the flat's exposure time.
5. **Defect correction after flat division** -- correct. Industry standard order.
6. **f64 accumulation for flat means** -- correct numerical practice.
7. **Divide-by-zero guard** -- `norm_flat > f32::EPSILON` check present.

### Minor: Defect Map Ordering Assumption

`DefectMap::correct()` iterates hot and cold indices and replaces each with the median of
neighbors. If two adjacent same-color pixels are both defective, the second correction reads
the already-corrected value of the first. This is a known issue in sequential correction.

- **Impact:** Negligible. Hot pixels are sparse (typically <0.1% of pixels). Adjacent
  same-color defects (stride-2 for Bayer) are extremely rare. The median of N-1 good
  neighbors is still robust even if one neighbor was previously corrected.

### Minor: lower threshold clamps to 0.0

In `compute_per_color_thresholds`, the lower threshold is: `(median - sigma_threshold * sigma).max(0.0)`.
For normalized data in [0,1] range this is fine. But if data contains negative values
(e.g., after subtraction), pixels below zero are never flagged as cold.

- **Impact:** Negligible. The defect map is built from master darks (before subtraction),
  which have non-negative values.

---

## Unnecessary Complexity

### None Identified

The module is lean and well-structured:

- **`stack_cfa_frames`** is a thin wrapper that delegates to the existing stacking pipeline.
  The < 8 frame fallback to median is a pragmatic choice.
- **`CalibrationMasters::calibrate()`** is 15 lines with clear step ordering.
- **`DefectMap::from_master_dark()`** separates concerns cleanly: threshold computation
  (per-color) is in its own function, sample collection is separate, correction is separate.
- **CFA-aware correction** has three clear paths (Mono, Bayer, X-Trans) with appropriate
  neighbor selection for each.
- **Adaptive sampling** is a reasonable optimization to avoid O(n log n) sort on 24M pixels.

The only borderline complexity is `collect_color_samples` which allocates the full channel
Vec before subsampling for CFA mode (see Low issue above), but this is a performance issue
not a complexity issue.

---

## Recommendations

### Short-term (easy, high value) -- POSTPONED

1. **Expose sigma threshold parameter** -- Allow `CalibrationMasters::new()` and
   `from_raw_files()` to accept a sigma threshold (default 5.0). Trivial change, high
   user value.
2. **Strided CFA sampling** -- Fix `collect_color_samples` to use strided iteration for
   CFA mode (count pixels first, compute stride, collect every Nth). Saves ~24MB per color
   channel on large images.

### Medium-term (moderate effort) -- POSTPONED

3. **Flat-based cold pixel detection** -- Add `DefectMap::from_master_flat()` that flags
   pixels below `threshold * median(flat)` per CFA channel. Merge with dark-based map.
   APP uses 50% as default threshold.
4. **Apply defect map to master flat** -- Correct defective pixels in the master flat
   after stacking but before normalization.
5. **Parallel master stacking** -- Use `rayon::join` or `std::thread::scope` to stack
   darks, flats, biases, and flat-darks in parallel in `from_raw_files()`.

### Long-term (if needed for CCD support) -- POSTPONED

6. **Dark frame scaling** -- `dark_scaled = bias + (dark - bias) * t_light / t_dark`.
   Requires separate bias frame. Useful for CCD dark libraries.
7. **Dark optimization** -- Numeric optimization of dark scaling factor to minimize
   residual noise (a la PixInsight). More robust than simple exposure scaling.

---

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

---

## References

- [PixInsight Master Frames tutorial](https://www.pixinsight.com/tutorials/master-frames/)
- [PixInsight dark optimization algorithm](https://pixinsight.com/forum/index.php?threads/dark-frame-optimization-algorithm.8529/)
- [PixInsight CFA flat scaling discussion](https://pixinsight.com/forum/index.php?threads/when-to-check-enable-cfa-separate-cfa-flat-and-dslr-cmos-calibration-needs.16397/)
- [PixInsight CosmeticCorrection guide](https://chaoticnebula.com/cosmetic-correction/)
- [Siril calibration docs (1.5.0)](https://siril.readthedocs.io/en/latest/preprocessing/calibration.html)
- [Siril cosmetic correction docs (1.5.0)](https://siril.readthedocs.io/en/latest/processing/cc.html)
- [Astropy CCD Reduction Guide](https://www.astropy.org/ccd-reduction-and-photometry-guide/)
- [ccdproc reduction toolbox](https://ccdproc.readthedocs.io/en/latest/reduction_toolbox.html)
- [APP bad pixel map tutorial](https://www.astropixelprocessor.com/community/tutorials-workflows/creating-a-bad-pixel-map/)
- [CMOS bias frames and scaled darks (AAVSO)](https://www.aavso.org/bias-frames-and-cmos-cameras-scaled-and-unscaled-darks)
- [CMOS calibration workflow](https://astrobasics.de/en/basics/bias-flats-darks-darkflats/)
- [Dark subtraction negative values](https://www.cloudynights.com/topic/638193-dark-subtraction-do-not-cut-off-negative-values/)
- [Flat-field correction (Wikipedia)](https://en.wikipedia.org/wiki/Flat-field_correction)
- [MAD for outlier detection](https://en.wikipedia.org/wiki/Median_absolute_deviation)
- [Cosmetic correction timing (Telescope Live)](https://telescope.live/blog/when-best-time-do-cosmetic-correction-and-linear-defect-removal)
