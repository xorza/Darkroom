# Stacking Module: Pending Fixes and Improvements

## Bugs

### 1. `SigmaClipAsymmetric` dispatches to `LinearFitClipConfig`
**Files**: `stack.rs:167`, `stack.rs:217`
**Severity**: High — silent wrong behavior

`apply_rejection()` and `apply_rejection_weighted()` dispatch `Rejection::SigmaClipAsymmetric` to `linear_fit_clipped_mean()`. Asymmetric sigma clipping should clip from the **median** (like regular sigma clip with separate low/high thresholds), not from a linear fit. These are fundamentally different:
- Sigma clip: center = median, reject if deviation from median exceeds threshold
- Linear fit: center = fitted line `a + b*x`, reject if residual from fit exceeds threshold

**Fix**: Add `sigma_clipped_mean_asymmetric()` to `rejection.rs` that clips from median with separate `sigma_low`/`sigma_high` thresholds. Update dispatch in `stack.rs`.

### 2. Weighted percentile ignores weights
**Files**: `stack.rs:243`
**Severity**: Medium — silent data loss

Comment says "Percentile sorts the array, weights become misaligned / Use unweighted result". Weights are silently ignored.

**Fix**: Sort (value, weight) pairs together so weights track their values after sorting.

### 3. Weighted winsorized ignores weights
**Files**: `stack.rs:230`
**Severity**: Medium — silent data loss

`winsorized_sigma_clipped_mean()` takes `&[f32]` (no weights). The weighted code path calls it and returns the unweighted result.

**Fix**: Add `winsorized_sigma_clipped_mean_weighted()` that applies weighted mean to the winsorized values.

### 4. Sigma clipping uses stddev instead of MAD
**Files**: `rejection.rs:219`
**Severity**: Low — suboptimal robustness

Standard deviation is inflated by the very outliers sigma clipping tries to reject. MAD (Median Absolute Deviation) is the industry standard scale estimator for iterative clipping (used by Siril, PixInsight, and already implemented in `math::sigma_clipped_median_mad`).

**Fix**: Replace `sum_squared_diff / n` with MAD-based scale estimate in `sigma_clipped_mean()`.

## Missing Features

### 5. Global normalization not implemented
**Files**: `stack.rs`
**Severity**: High — advertised but non-functional

`Normalization::Global` exists in the enum but `stack_with_progress()` never reads the `normalization` field. Frames are stacked raw regardless of setting.

**Fix**: Before stacking, compute global median/scale for each frame and normalize. Apply in the cache loading phase or as a pre-processing step.

### 6. Local normalization not integrated
**Files**: `local_normalization.rs`, `stack.rs`
**Severity**: Medium — fully implemented, just not wired in

`local_normalization.rs` is complete with tests but everything is `#[allow(dead_code)]`. The stacking pipeline doesn't call it.

**Fix**: Wire `LocalNormalizationMap` into `stack_with_progress()` when `Normalization::Local` is configured.

### 7. No auto frame weighting
**Files**: `config.rs`
**Severity**: Low — convenience feature

Only manual weight specification. Siril offers FWHM/noise/star-count weighting. PixInsight offers SNR/noise weighting.

**Fix**: Add `StackConfig::auto_weighted(frame_qualities: &[FrameQuality])` preset that computes weights from detection results.

## Code Quality

### 8. DESIGN-PROPOSAL.md is stale
The unified API it proposed has been fully implemented. The document references the old API (`ImageStack`, `StackingMethod`, etc.) which no longer exists.

**Fix**: Delete `DESIGN-PROPOSAL.md`.
