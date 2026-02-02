//! Math utilities with SIMD acceleration.
//!
//! Provides optimized math operations with platform-specific SIMD for ARM NEON (aarch64) and x86 SSE4.
//!
//! # Modules
//!
//! - [`sum`]: Sum, accumulate, and scale operations
//! - [`statistics`]: Median, MAD, and sigma-clipped statistics
//! - [`deviation`]: Absolute deviation computation

// =============================================================================
// Submodules
// =============================================================================

mod bbox;
pub mod deviation;
pub mod statistics;
pub mod sum;
mod vec2;

// Keep scalar module at top level for backwards compatibility
pub mod scalar {
    pub use super::deviation::scalar::*;
    pub use super::sum::scalar::*;
}

// =============================================================================
// Re-exports
// =============================================================================

pub use bbox::Aabb;
pub use vec2::Vec2us;

// Re-export commonly used functions at top level for convenience
pub use deviation::abs_deviation_inplace;
pub use statistics::{
    MAD_TO_SIGMA, mad_f32_with_scratch, mad_to_sigma, median_and_mad_f32_mut, median_f32_mut,
    sigma_clipped_median_mad, sigma_clipped_median_mad_arrayvec,
};
pub use sum::{accumulate, mean_f32, scale, sum_f32, sum_squared_diff};

// =============================================================================
// Constants
// =============================================================================

/// FWHM to Gaussian sigma conversion factor.
///
/// For a Gaussian distribution, FWHM = 2√(2ln2) × σ ≈ 2.3548 × σ.
/// This is the exact value: 2 * sqrt(2 * ln(2)).
pub const FWHM_TO_SIGMA: f32 = 2.354_82;

// =============================================================================
// Unit Conversion Functions
// =============================================================================

/// Convert FWHM to Gaussian sigma.
#[inline]
pub fn fwhm_to_sigma(fwhm: f32) -> f32 {
    fwhm / FWHM_TO_SIGMA
}

/// Convert Gaussian sigma to FWHM.
#[inline]
pub fn sigma_to_fwhm(sigma: f32) -> f32 {
    sigma * FWHM_TO_SIGMA
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fwhm_sigma_conversion_roundtrip() {
        let fwhm = 4.5;
        let sigma = fwhm_to_sigma(fwhm);
        let fwhm_back = sigma_to_fwhm(sigma);
        assert!((fwhm - fwhm_back).abs() < 1e-6);
    }

    #[test]
    fn test_fwhm_to_sigma_known_value() {
        let sigma = fwhm_to_sigma(FWHM_TO_SIGMA);
        assert!((sigma - 1.0).abs() < 1e-6);
    }
}
