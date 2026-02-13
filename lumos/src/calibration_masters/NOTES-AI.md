# calibration_masters Module - Code Review vs Industry Standards

## Overview
Creates master calibration frames (dark, flat, bias) from raw CFA sensor data. Detects hot pixels via MAD-based statistics. Calibrates light frames with dark subtraction, flat division, CFA-aware hot pixel correction. Operates on raw data before demosaicing.

## What It Does Well
- Correct calibration formula: (Light - Dark) / normalize(Flat - Bias)
- Robust hot pixel detection with MAD (correct 1.4826 constant)
- CFA-aware correction: stride-2 for Bayer, color-matching for X-Trans (better than Siril)
- Adaptive stacking: median (<8 frames) vs sigma-clipped mean (>=8)
- Sigma floor prevents over-detection on well-stacked darks
- Strong test coverage including full pipeline algebraic verification
- Well-documented README with references

## Issues Found

### High: No Cold/Dead Pixel Detection
- File: hot_pixels.rs:73-79
- Only detects pixels ABOVE threshold (hot pixels)
- No detection of dead pixels (zero/low response) below threshold
- Dead pixels are multiplicative defects that survive dark subtraction
- Siril and PixInsight both detect hot AND cold pixels
- The synthetic test infra already has `add_dead_pixels` function
- Fix: Add lower threshold `median - sigma_threshold * sigma`

### High: Dark Subtraction Does Not Clamp to Zero
- File: cfa.rs:158-160
- `*l -= d` can produce negative values
- PixInsight and Siril clamp results at zero
- Negatives propagate through flat division and hot pixel correction
- Fix: `*l = (*l - d).max(0.0)` or document that negatives are intentional

### Medium: README Claims Per-Channel Analysis But Code Does Single-Channel
- File: README.md, hot_pixels.rs:109
- README says "Per-channel analysis for RGB data"
- Code computes statistics across ALL pixels regardless of CFA channel
- Green pixels (50%) can have different dark current than R/B
- Fix: Either implement per-CFA-channel MAD or correct the README

### Medium: No Flat Dark Support
- File: mod.rs:96-103
- No slot for flat darks (dark frames at flat exposure time)
- Noted as limitation in README
- Important for narrowband with longer flat exposures
- DeepSkyStacker, Siril, PixInsight all support flat darks

### Low: X-Trans Neighbor Search Biased Toward Top-Left
- File: hot_pixels.rs:253-271
- Iterates dy/dx from negative to positive, breaks at 24
- Bottom-right neighbors less likely to be included
- Fix: Sort candidates by distance, take closest 24

### Low: HotPixelMap::correct is Sequential
- File: hot_pixels.rs:93-106
- Could be parallelized for large defect maps (>100K on 24MP sensor)
- Bayer stride-2 neighbors don't overlap, parallel correction is safe

### Documented Limitations (acknowledged in README)
- No dark frame scaling for different exposure times/temperatures
- No dark flat support
