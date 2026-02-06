# Stacking Module: Pending Fixes and Improvements

## Fixed

### 1. `SigmaClipAsymmetric` dispatched to `LinearFitClipConfig` (FIXED)
Added `sigma_clipped_mean_asymmetric()` and `AsymmetricSigmaClipConfig` to `rejection.rs`. Updated dispatch in `stack.rs` to use median-based clipping instead of linear fit.

### 2. Weighted percentile ignored weights (FIXED)
`apply_rejection_weighted` now sorts (value, weight) pairs together and computes weighted mean of the retained range.

### 3. Weighted winsorized ignored weights (FIXED)
Extracted `winsorize()` function from `winsorized_sigma_clipped_mean()`. Weighted path now winsorizes values then computes weighted mean.

### 4. Sigma clipping used stddev instead of MAD (FIXED)
`sigma_clipped_mean`, `sigma_clipped_mean_asymmetric`, and `winsorize` now use MAD-based scale estimation (`mad_f32_with_scratch` + `mad_to_sigma`). MAD is robust to the outliers being rejected, unlike stddev.

## Remaining

### 5. Global normalization not implemented
**Severity**: High — `Normalization::Global` exists in the enum but `stack_with_progress()` never applies it.

**Fix**: Before stacking, compute global median/scale for each frame and normalize. Apply in the cache loading phase or as a pre-processing step.

### 6. Local normalization not integrated
**Severity**: Medium — `local_normalization.rs` is fully implemented and tested but not wired into the stacking pipeline. All code is `#[allow(dead_code)]`.

**Fix**: Wire `LocalNormalizationMap` into `stack_with_progress()` when `Normalization::Local` is configured.

### 7. No auto frame weighting
**Severity**: Low — convenience feature. Only manual weight specification currently.

**Fix**: Add `StackConfig::auto_weighted(frame_qualities: &[FrameQuality])` preset that computes weights from detection results (SNR, FWHM, noise, star count).
