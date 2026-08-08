//! Winsorized sigma clipping: pull outliers to the clip bounds and re-estimate spread until it
//! converges, then clip on the bias-corrected estimate. More robust than plain sigma clipping on
//! small stacks.

use crate::error::InvalidConfigField;
use crate::stacking::combine::cache::ScratchBuffers;
use crate::stacking::combine::rejection::{compact_within, reset_indices, validate_sigma_bounds};

/// Configuration for winsorized sigma clipping.
///
/// Two-phase algorithm matching PixInsight/Siril:
/// 1. **Robust estimation**: Iteratively Winsorize with Huber's c=1.5 constant
///    until sigma converges, then apply 1.134 bias correction to get robust
///    (center, sigma) estimates.
/// 2. **Rejection**: Standard sigma clipping using the robust estimates and
///    user-specified sigma_low/sigma_high thresholds.
///
/// This is more robust for small sample sizes than standard sigma clipping.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WinsorizedClipConfig {
    /// Sigma threshold for low outliers (below median).
    pub sigma_low: f32,
    /// Sigma threshold for high outliers (above median).
    pub sigma_high: f32,
}

/// Huber's constant for Winsorization boundaries.
const HUBER_C: f32 = 1.5;
/// Bias correction factor for Winsorized standard deviation.
const WINSORIZED_CORRECTION: f32 = 1.134;
/// Convergence threshold for iterative Winsorization.
const WINSORIZE_CONVERGENCE: f32 = 0.0005;
/// Maximum iterations for Winsorization convergence.
const WINSORIZE_MAX_ITER: u32 = 50;

impl Default for WinsorizedClipConfig {
    fn default() -> Self {
        Self {
            sigma_low: 2.5,
            sigma_high: 2.5,
        }
    }
}

impl WinsorizedClipConfig {
    pub fn new(sigma: f32) -> Self {
        Self {
            sigma_low: sigma,
            sigma_high: sigma,
        }
    }

    pub fn new_asymmetric(sigma_low: f32, sigma_high: f32) -> Self {
        Self {
            sigma_low,
            sigma_high,
        }
    }

    /// Validate the clip thresholds.
    pub fn validate(&self) -> Result<(), InvalidConfigField> {
        validate_sigma_bounds(self.sigma_low, self.sigma_high)
    }

    /// Phase 1: Iteratively Winsorize to get a robust [`WinsorizedEstimate`].
    ///
    /// Uses Huber's c=1.5 for Winsorization boundaries, converges when
    /// `|sigma_new - sigma_old| / sigma_old < 0.0005`. Applies 1.134 bias
    /// correction to the final sigma.
    ///
    /// `working` is sorted **once** up front: Winsorization clamps every value into
    /// `[low, high]`, a monotonic map, so a sorted buffer stays sorted across iterations.
    /// The median is then the middle element (O(1)) every pass — replacing the per-iteration
    /// quickselect + buffer copy that dominated this hot path. `winsorized_stddev` is an
    /// order-independent sum, so sorting changes neither the center nor the sigma.
    pub(super) fn robust_estimate(
        &self,
        values: &[f32],
        working: &mut Vec<f32>,
    ) -> WinsorizedEstimate {
        working.clear();
        working.extend_from_slice(values);
        working.sort_unstable_by(f32::total_cmp);

        // `select_nth_unstable`'s median (index len/2) equals the sorted element at that index,
        // so `working[mid]` reproduces the previous `median_f32_fast` result exactly.
        let mid = working.len() / 2;
        let mut center = working[mid];
        let mut sigma = winsorized_stddev(working, center) * WINSORIZED_CORRECTION;

        if sigma < f32::EPSILON {
            return WinsorizedEstimate { center, sigma: 0.0 };
        }

        for _ in 0..WINSORIZE_MAX_ITER {
            let low_bound = center - HUBER_C * sigma;
            let high_bound = center + HUBER_C * sigma;

            // Clamp outliers to the boundary values. `low_bound <= high_bound` (sigma > 0), and
            // a monotone clamp preserves the existing sort order, so no re-sort is needed.
            for v in working.iter_mut() {
                *v = v.clamp(low_bound, high_bound);
            }

            center = working[mid];
            let sigma_new = winsorized_stddev(working, center) * WINSORIZED_CORRECTION;

            if sigma_new < f32::EPSILON {
                return WinsorizedEstimate { center, sigma: 0.0 };
            }

            let converged = (sigma_new - sigma).abs() <= sigma * WINSORIZE_CONVERGENCE;
            sigma = sigma_new;

            if converged {
                break;
            }
        }

        WinsorizedEstimate { center, sigma }
    }

    /// Phase 2: Reject outliers using the robust estimate from phase 1.
    ///
    /// Standard sigma clipping with the [`WinsorizedEstimate`] and the user's
    /// sigma_low/sigma_high thresholds.
    pub(super) fn reject(&self, values: &mut [f32], scratch: &mut ScratchBuffers) -> usize {
        debug_assert!(!values.is_empty());

        reset_indices(&mut scratch.indices, values.len());

        if values.len() <= 2 {
            return values.len();
        }

        let estimate = self.robust_estimate(values, &mut scratch.estimate_values);

        if estimate.sigma < f32::EPSILON {
            return values.len();
        }

        let n = values.len();
        compact_within(
            values,
            &mut scratch.indices,
            n,
            self.sigma_low * estimate.sigma,
            self.sigma_high * estimate.sigma,
            |_| estimate.center,
        )
    }
}

/// Location and spread from [`WinsorizedClipConfig::robust_estimate`].
///
/// Deliberately not a [`MedianMad`](crate::math::statistics::MedianMad): `sigma` is a
/// bias-corrected sample standard deviation about the Winsorized center — see
/// [`winsorized_stddev`] — so it is already in Gaussian units, and sending it through that
/// type's MAD rescale would silently inflate every Winsorized threshold by 1.4826.
#[derive(Debug, Clone, Copy)]
pub(super) struct WinsorizedEstimate {
    pub(super) center: f32,
    pub(super) sigma: f32,
}

/// Sample standard deviation of `values` about the given `center` (not MAD) — the spread estimate
/// the Winsorized robust loop iterates on.
pub(super) fn winsorized_stddev(values: &[f32], center: f32) -> f32 {
    let n = values.len() as f32;
    if n <= 1.0 {
        return 0.0;
    }
    let variance = values
        .iter()
        .map(|&v| {
            let d = v - center;
            d * d
        })
        .sum::<f32>()
        / (n - 1.0);
    variance.sqrt()
}
