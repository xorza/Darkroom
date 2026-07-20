//! Math utilities.
//!
//! # Modules
//!
//! - [`sum`]: Sum, accumulate, and scale operations
//! - [`statistics`]: Median, MAD, and sigma-clipped statistics

pub(crate) mod dmat3;
pub(crate) mod rect;
pub(crate) mod vec2us;

pub(crate) mod statistics;
pub(crate) mod sum;

/// FWHM to Gaussian sigma conversion factor.
///
/// For a Gaussian distribution, FWHM = 2√(2ln2) × σ ≈ 2.3548 × σ.
/// This is the exact value: 2 * sqrt(2 * ln(2)).
pub(crate) const FWHM_TO_SIGMA: f32 = 2.354_82;

/// Convert FWHM to Gaussian sigma.
#[inline]
pub(crate) fn fwhm_to_sigma(fwhm: f32) -> f32 {
    fwhm / FWHM_TO_SIGMA
}

/// Convert Gaussian sigma to FWHM.
#[inline]
pub(crate) fn sigma_to_fwhm(sigma: f32) -> f32 {
    sigma * FWHM_TO_SIGMA
}

#[cfg(test)]
mod tests {
    use crate::math::*;

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
