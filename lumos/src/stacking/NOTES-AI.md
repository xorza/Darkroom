# stacking Module - Code Review vs Industry Standards

## Overview
Astronomical image stacking: Mean/Median combination, rejection algorithms (sigma clipping symmetric/asymmetric, winsorized, linear fit, percentile, GESD), normalization (none/global/multiplicative), auto memory management (in-memory vs mmap), per-frame weighting.

## What It Does Well
- MAD-based sigma for all rejection (correct 1.4826 factor, matches PixInsight/Siril)
- Asymmetric sigma clipping (independent sigma_low/sigma_high)
- Index tracking through rejection for weighted stacking
- Memory-efficient disk-backed mode with adaptive chunking (75% RAM threshold)
- Per-channel planar storage reduces working set by 3x
- Linear fit clipping with robust initial pass (median+MAD)
- Global normalization matches Siril "additive with scaling"
- Good test coverage including cross-validation between algorithms

## Issues Found

### Medium: GESD Lacks Asymmetric Rejection (Relaxation Parameter)
- File: rejection.rs:344-414
- PixInsight's ESD has "relaxation" parameter (default 1.5) multiplying sigma for low pixels
- Current implementation uses same test statistic both directions
- Fix: Add `low_relaxation: f32` to GesdConfig

### Medium: GESD Uses Median+MAD, PixInsight Uses Trimmed Mean+Trimmed Stddev
- File: rejection.rs:371-381
- PixInsight trims 30% high, 20% low, then mean/stddev of remaining 50%
- Different sensitivity characteristics from median+MAD

### Medium: Winsorized Clipping Uses Symmetric Sigma Only
- File: rejection.rs:136-140
- Single `sigma` field; PixInsight/Siril support separate sigma_low/sigma_high
- Fix: Add sigma_low/sigma_high like SigmaClipConfig

### Medium: Linear Fit Clipping Center Uses Midpoint Position
- File: rejection.rs:273
- `center = a + b * (n / 2.0)` - uses fit value at median position
- PixInsight uses per-pixel fitted values for center
- Simplification may cause incorrect rejection at extremes

### Medium: Linear Fit MAD-of-Residuals Uses Wrong Centering
- File: rejection.rs:277-282
- Subtracts residual at median position, not median of all residuals
- Should compute MAD of residuals directly

### Medium: Missing Additive-Only Normalization
- File: config.rs:34-42
- Has None, Global (additive+scaling), Multiplicative
- Missing pure additive (shift by ref_median - frame_median, no gain)
- Useful for same-setup frames with different sky backgrounds

### Medium: Default Normalization is None, README Recommends Global for Lights
- File: config.rs:40, README.md
- Users using default config for light frames get suboptimal results
- Industry practice defaults normalization ON for lights

### Low: Missing Separate Normalization for Rejection vs Combination
- PixInsight separates rejection normalization from final combination normalization
- Important for preserving photometric accuracy

### Low: Missing Rejection Maps / Diagnostic Output
- PixInsight/Siril generate per-pixel high/low rejection count maps
- Critical for diagnosing parameter aggressiveness

### Low: No Automatic Frame Weighting
- Only manual weights; PixInsight computes noise-based weights (1/variance^2)
- README notes auto-weighting was "evaluated and removed"

### Low: compute_channel_stats is Single-Threaded
- File: cache.rs:286-303
- 60 full-frame median computations for 20 RGB frames
- Could parallelize across frames/channels via rayon

### Low: Insertion Sort in Linear Fit/Percentile for Large Stacks
- File: rejection.rs:249-255, 321-327
- O(n^2) for >50 frames; sort_unstable faster for n > ~30
