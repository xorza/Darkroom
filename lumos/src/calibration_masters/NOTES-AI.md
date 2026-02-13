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

## Defective Pixel Detection

**Algorithm:** MAD (Median Absolute Deviation) with sigma threshold.
- `sigma = MAD * 1.4826` (correct conversion constant for normal distribution)
- Hot threshold: `median + sigma_threshold * sigma`
- Cold threshold: `max(median - sigma_threshold * sigma, 0.0)`
- Default threshold: 5.0 sigma
- Sigma floor: `max(computed_sigma, median * 0.1)` prevents over-detection on
  uniform darks where MAD approaches zero
- `DefectMap` stores separate `hot_indices` and `cold_indices` vectors
- Per-CFA-color statistics: each pixel tested against its own color's thresholds
  (R, G, B computed independently for Bayer/X-Trans; single channel for Mono)

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
outlier detection because it is robust to the outliers themselves. Per-CFA-color
statistics is better than most tools (Siril uses global statistics). The 5-sigma
default is conservative, which avoids false positives but may miss marginal hot pixels.

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

## Issues

### ~~Medium: Sigma Floor Fails When Median is Zero~~ — FIXED
- Added absolute floor: `sigma = computed_sigma.max(median * 0.1).max(1e-4)`
- `1e-4` in [0,1] range ≈ 6.5 ADU in 16-bit, prevents over-detection while still
  catching genuine hot pixels. Test: `test_defect_detection_zero_median_no_false_positives`

### Low: collect_color_samples Allocates Full Channel Before Subsampling
- **File:** defect_map.rs:229-241
- For CFA mode, collects ALL pixels of target color into a Vec, then subsamples.
  On a 6000x4000 image, this allocates ~6M f32s (~24MB) per color channel just to
  keep 100K samples.
- The mono path already uses strided sampling directly (line 225).
- **Fix:** Use strided iteration for CFA too — count matching pixels first, compute
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
- For CMOS sensors, scaling is unreliable (amp glow, non-linear dark current).
- Modern practice: Use matched darks (same exposure/temp) rather than scaling.
- **Status:** Acceptable limitation for CMOS workflows.

## Documentation Issues

### README.md Outdated
- Line 42 says "No flat dark support" but the code fully supports flat darks
  (`master_flat_dark` field, `from_raw_files` accepts flat_darks, flat dark takes
  priority over bias for flat normalization).
- Should be updated to reflect current implementation.

## Test Coverage

- Unit tests for hot pixel detection (small/large images, edge cases, no defects)
- Unit tests for cold/dead pixel detection and mixed hot+cold detection
- Unit tests for correction (Bayer stride-2, Mono 8-connected, corner pixels, cold pixels)
- Per-CFA-color detection tests (hot red in mixed-value Bayer, cold blue)
- Integration tests for full calibration pipeline (dark sub, bias-only, flat correction,
  combined dark+flat+bias with algebraic verification, flat dark priority over bias)
- Dimension mismatch assertions tested with `#[should_panic]`
- Full pipeline test verifies vignetting cancellation algebraically
- **Missing:** No test for median=0 edge case in sigma floor

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
