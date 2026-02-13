# calibration_masters Module

## Overview

Creates master calibration frames (dark, flat, bias) from raw CFA sensor data and
applies them to calibrate light frames. Detects hot pixels via MAD-based statistics.
Operates on raw single-channel CFA data before demosaicing.

**Files:** mod.rs (orchestration), hot_pixels.rs (detection/correction), tests.rs (integration tests)

## Architecture

```
CalibrationMasters::from_raw_files()     CalibrationMasters::new()
  stack_cfa_frames(darks, ...)             Takes pre-built CfaImages
  stack_cfa_frames(flats, ...)             Generates HotPixelMap from dark
  stack_cfa_frames(biases, ...)
         |                                          |
         v                                          v
CalibrationMasters { master_dark, master_flat, master_bias, hot_pixel_map }
         |
         v
calibrate(light):
  1. Dark subtraction (or bias-only fallback)     CfaImage::subtract()
  2. Flat division with normalization              CfaImage::divide_by_normalized()
  3. CFA-aware hot pixel correction                HotPixelMap::correct()
```

## Calibration Formula

**Implementation:** `calibrated = (Light - Dark) / normalize(Flat - Bias)`

Where `normalize(X) = X / mean(X)`, so flat division preserves the light frame's
intensity scale. When bias is provided, flat normalization uses `mean(Flat - Bias)`.

**Industry standard (Siril, PixInsight, Astropy CCD Guide):**
`L_c = (L - D) / (F - O)` with normalization by `mean(F - O)`.

**Verdict:** The formula is correct. The implementation matches the standard. When both
dark and bias exist, dark is subtracted from the light (dark already contains bias),
and bias is subtracted from the flat before normalization. This is the correct approach
because a master dark captured at the same exposure time already includes the bias signal.

The flat division guards against divide-by-zero with `norm_flat > f32::EPSILON` and
uses `f64` accumulation for the mean, which is good numerical practice.

## Hot Pixel Detection

**Algorithm:** MAD (Median Absolute Deviation) with sigma threshold.
- `sigma = MAD * 1.4826` (correct conversion constant for normal distribution)
- `threshold = median + sigma_threshold * sigma`
- Default threshold: 5.0 sigma
- Sigma floor: `max(computed_sigma, median * 0.1)` prevents over-detection on
  uniform darks where MAD approaches zero

**Industry comparison:**
- PixInsight CosmeticCorrection: Uses master dark or auto-detect (3 sigma default,
  based on local window statistics). Replaces with average of surrounding pixels.
- Siril: Uses average deviation (avgDev) not MAD. Detects via
  `m_5x5 + max(avgDev, sigma_high * avgDev)`. Replaces hot pixels with 3x3 average,
  cold pixels with 5x5 median.
- Astropy CCD Guide: Identifies hot pixels by comparing dark current across different
  exposure times; flags pixels above 4 e-/sec.
- Common sigma range: 3-5 sigma (this implementation uses 5.0, conservative).

**Verdict:** MAD is a better choice than standard deviation or average deviation for
outlier detection because it is robust to the outliers themselves. The 5-sigma default
is conservative, which avoids false positives but may miss marginal hot pixels.

## Hot Pixel Correction (CFA-Aware)

**Bayer:** Median of up to 8 same-color neighbors at stride-2 offsets.
**X-Trans:** Median of up to 24 same-color neighbors within radius 6.
**Mono:** Median of 8-connected neighbors.

**Industry comparison:** PixInsight uses average of surrounding pixels. Siril uses
average for hot, median for cold. Median is more robust to nearby defects and is the
preferred choice for scientific data.

**Verdict:** CFA-aware correction at stride-2 for Bayer is correct and better than tools
that operate post-demosaic (e.g., some Siril modes). Pre-demosaic correction avoids
color artifacts from interpolating across filter boundaries.

## Adaptive Sampling

For images >200K pixels, statistics are computed on 100K uniformly sampled pixels.
This gives <0.5% median error with >99% confidence per CLT for order statistics.
Avoids O(n log n) sort on full-resolution images.

## Stacking Strategy

- < 8 frames: median (robust to outliers with few frames)
- >= 8 frames: sigma-clipped mean at 3 sigma (statistically efficient)
- Darks/biases: no normalization; Flats: multiplicative normalization

## Issues Found

### High: No Cold/Dead Pixel Detection
- **File:** hot_pixels.rs:64-69
- Only detects pixels ABOVE threshold (hot pixels)
- No detection of dead pixels (zero/low response) that fall below
  `median - sigma_threshold * sigma`
- Dead pixels are multiplicative defects (zero or reduced sensitivity) that survive
  dark subtraction. They appear as dark spots in the final image.
- Both PixInsight and Siril detect hot AND cold pixels as a pair
- **Fix:** Add lower threshold check: `val < median - sigma_threshold * sigma`

### High: Dark Subtraction Does Not Clamp Negatives
- **File:** cfa.rs:157-160 (`*l -= d` with no floor)
- Can produce negative pixel values when dark noise exceeds light signal
- **Industry approaches:**
  - PixInsight: Adds a pedestal (e.g., +100 ADU) before calibration to keep values
    positive, stored in FITS PEDESTAL keyword. Allows lossless floating-point workflow.
  - Siril: Warns when light median is close to dark median. Processes in float but
    negative values can cause issues downstream.
  - MaxIm DL: Adds fixed pedestal (typically 100 ADU)
  - Cloudy Nights consensus: Clamping to zero loses information (creates bright bias
    in stacked result). Preserving negatives in float is preferred if pipeline handles it.
- **Current impact:** Since the pipeline uses f32 throughout and stacking happens after
  calibration, negative values from noise are averaged out during integration. This is
  actually the correct floating-point approach. However, if images are ever exported to
  integer formats between calibration and stacking, clamping would destroy information.
- **Recommendation:** Document that negatives are intentional and the pipeline relies on
  float arithmetic. Consider adding optional pedestal support for integer export paths.

### Medium: Single-Channel Statistics vs Per-CFA-Channel
- **File:** hot_pixels.rs:49-52 (calls `compute_single_channel_stats` on all pixels)
- Statistics are computed across ALL CFA pixels regardless of color channel
- In Bayer data, green pixels are 50% of the total and can have different dark current
  characteristics than red or blue pixels due to different spectral response and
  potentially different photodiode sizes in some sensor designs
- A hot pixel in the red channel might not exceed the global threshold if green pixels
  dominate the statistics
- **Industry:** PixInsight CosmeticCorrection has a CFA-aware mode. Siril supports
  CFA-specific processing.
- **Fix:** Compute separate MAD statistics for each CFA color channel (R, G1, G2, B for
  Bayer; 3 channels for X-Trans). Flag pixels hot within their own channel.

### Medium: No Flat Dark Support
- **File:** mod.rs:101-109
- No slot for flat darks (dark frames taken at flat exposure time)
- Important for narrowband imaging where flat exposures can be several seconds,
  accumulating non-negligible dark current
- Siril, PixInsight, and DeepSkyStacker all support flat darks
- **Fix:** Add optional `flat_darks` parameter to `from_raw_files()`. Subtract master
  flat dark from master flat before normalization.

### Low: X-Trans Neighbor Search Biased Toward Top-Left
- **File:** hot_pixels.rs:262-285
- Iterates dy then dx from negative to positive, breaks at 24 neighbors
- Systematic bias: top-left neighbors always included, bottom-right may be excluded
- **Fix:** Collect all same-color neighbors within radius, sort by Manhattan distance,
  take closest 24

### Low: HotPixelMap::correct is Sequential
- **File:** hot_pixels.rs:106-116
- Iterates `&self.indices` sequentially
- Safe to parallelize: Bayer stride-2 neighbors never overlap with other hot pixels'
  replacement zones (hot pixels are sparse). For X-Trans, radius-6 neighborhoods could
  theoretically overlap but in practice hot pixels are rare enough.
- **Fix:** Use `par_iter` with index-based writes (requires unsafe or par_chunks on
  the pixel buffer). Or partition indices by spatial locality.

### Low: No Dark Frame Scaling
- No support for scaling dark frames to different exposure times or temperatures
- Dark current scales linearly with exposure for CCDs: `dark_current = rate * time`
- For CMOS sensors, scaling is unreliable (amp glow, non-linear dark current behavior)
- Modern practice: Use matched darks (same exposure/temp) rather than scaling
- **Status:** Acceptable limitation for CMOS workflows. Document that matched darks are
  required.

## Test Coverage

- Unit tests for detection (small/large images, edge cases, no hot pixels)
- Unit tests for correction (Bayer stride-2, Mono 8-connected, corner pixels)
- Integration tests for full calibration pipeline (dark sub, bias-only, flat correction,
  combined dark+flat+bias with algebraic verification)
- Dimension mismatch assertions tested with `#[should_panic]`
- Full pipeline test verifies vignetting cancellation algebraically

## Key Constants and Thresholds

| Constant | Value | Source |
|----------|-------|--------|
| DEFAULT_HOT_PIXEL_SIGMA | 5.0 | Conservative; industry range is 3-5 |
| MAD_TO_SIGMA | 1.4826 | 1/Phi^-1(0.75), standard for normal distributions |
| MAX_MEDIAN_SAMPLES | 100,000 | Adaptive sampling for images >200K pixels |
| Sigma floor | median * 0.1 | Prevents MAD=0 over-detection on uniform darks |
| Stacking threshold | 8 frames | Below: median; above: sigma-clipped mean at 3.0 |

## References

- Siril calibration: https://siril.readthedocs.io/en/latest/preprocessing/calibration.html
- Siril cosmetic correction: https://siril.readthedocs.io/en/stable/processing/cc.html
- Astropy CCD Reduction Guide: https://www.astropy.org/ccd-reduction-and-photometry-guide/
- PixInsight CosmeticCorrection: https://chaoticnebula.com/cosmetic-correction/
- Dark frame negative values: https://www.cloudynights.com/topic/638193-dark-subtraction-do-not-cut-off-negative-values/
- AAVSO CMOS bias/darks: https://www.aavso.org/bias-frames-and-cmos-cameras-scaled-and-unscaled-darks
