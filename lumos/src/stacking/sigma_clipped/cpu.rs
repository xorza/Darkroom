//! CPU dispatch for sigma-clipped mean stacking.

use super::scalar;
use crate::stacking::SigmaClipConfig;

/// Calculate sigma-clipped mean, dispatching to best available implementation.
#[inline]
pub fn sigma_clipped_mean(values: &[f32], config: &SigmaClipConfig) -> f32 {
    // Sigma-clipped mean doesn't benefit from SIMD (requires sorting), use scalar
    scalar::sigma_clipped_mean(values, config)
}
