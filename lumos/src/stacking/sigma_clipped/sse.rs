//! SSE SIMD implementation for sigma-clipped mean stacking (x86_64).
//!
//! Note: Sigma-clipped mean involves median calculation and iterative clipping,
//! which don't benefit much from SIMD. This module is a placeholder for future
//! optimizations like vectorized variance calculation.

use super::scalar;
use crate::stacking::SigmaClipConfig;

/// Calculate sigma-clipped mean using SSE intrinsics.
/// Currently falls back to scalar implementation.
#[allow(dead_code)]
#[inline]
pub fn sigma_clipped_mean(values: &[f32], config: &SigmaClipConfig) -> f32 {
    // The iterative clipping algorithm doesn't vectorize well.
    // Future: could use SIMD for variance calculation step.
    scalar::sigma_clipped_mean(values, config)
}
