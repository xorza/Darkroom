//! FWHM conversions of the Gaussian and Moffat profiles.

/// The FWHM of a Gaussian per unit of its sigma: `2·√(2·ln 2)`.
///
/// `f64` is the canonical form, and [`FWHM_PER_SIGMA_F32`] is cast from it, so the two precisions
/// cannot round apart. `sqrt` is not `const`, so the digits are written out; a test holds them to
/// the expression.
pub(crate) const FWHM_PER_SIGMA: f64 = 2.354_820_045_030_949_3;

/// [`FWHM_PER_SIGMA`] in the precision the `f32` paths multiply in.
const FWHM_PER_SIGMA_F32: f32 = FWHM_PER_SIGMA as f32;

/// Convert FWHM to Gaussian sigma.
#[inline]
pub(crate) fn fwhm_to_sigma(fwhm: f32) -> f32 {
    fwhm / FWHM_PER_SIGMA_F32
}

/// Convert Gaussian sigma to FWHM.
#[inline]
pub(crate) fn sigma_to_fwhm(sigma: f32) -> f32 {
    sigma * FWHM_PER_SIGMA_F32
}

/// The FWHM of a 2-D Gaussian whose covariance has determinant `det` (px⁴): that of the circle of
/// equal area, whose σ is the geometric mean of the principal ones, `√(σ₁·σ₂) = det^¼`. The one
/// FWHM the moments and both profile fits report, so an elongated star reads the same width
/// whichever measured it.
pub(crate) fn equal_area_fwhm(det: f64) -> f32 {
    sigma_to_fwhm(det.max(0.0).sqrt().sqrt() as f32)
}

/// The FWHM of a Moffat profile, `2α·√(2^(1/β) − 1)`.
pub(crate) fn alpha_beta_to_fwhm(alpha: f32, beta: f32) -> f32 {
    2.0 * alpha * (2.0f32.powf(1.0 / beta) - 1.0).sqrt()
}

/// The Moffat α of a profile with this FWHM and `beta`: the inverse of [`alpha_beta_to_fwhm`].
pub(crate) fn fwhm_beta_to_alpha(fwhm: f32, beta: f32) -> f32 {
    fwhm / (2.0 * (2.0f32.powf(1.0 / beta) - 1.0).sqrt())
}

#[cfg(test)]
mod tests {
    use std::f64::consts::LN_2;

    use crate::math::fwhm::{
        FWHM_PER_SIGMA, FWHM_PER_SIGMA_F32, alpha_beta_to_fwhm, equal_area_fwhm,
        fwhm_beta_to_alpha, fwhm_to_sigma, sigma_to_fwhm,
    };

    #[test]
    fn the_constant_is_two_root_two_ln_two() {
        assert_eq!(FWHM_PER_SIGMA, 2.0 * (2.0 * LN_2).sqrt());
        assert_eq!(sigma_to_fwhm(1.0), FWHM_PER_SIGMA_F32);
        assert_eq!(fwhm_to_sigma(FWHM_PER_SIGMA_F32), 1.0);
    }

    /// A round Gaussian of σ 2 has `det = 2⁴`, and the FWHM is `2·2.3548`; one of σ 1 by 4 has the
    /// same area, and so the same FWHM. Exact powers of two through two square roots, then the
    /// f32 constant.
    #[test]
    fn equal_area_fwhm_is_the_geometric_mean_width() {
        assert_eq!(equal_area_fwhm(16.0), 2.0 * FWHM_PER_SIGMA_F32);
        assert_eq!(equal_area_fwhm(1.0 * 16.0), equal_area_fwhm(4.0 * 4.0));
        assert_eq!(equal_area_fwhm(-1.0), 0.0);
    }

    /// 4.5 → 4.5 / 2.35482 → back: a division and a multiplication by one f32 constant, each
    /// within half an ulp, so the round trip lands within one ulp of 4.5 (4.8e-7).
    #[test]
    fn fwhm_sigma_conversion_roundtrip() {
        let back = sigma_to_fwhm(fwhm_to_sigma(4.5));
        assert!((back - 4.5).abs() <= 4.5 * f32::EPSILON, "{back}");
    }

    /// FWHM = 2α·√(2^(1/β) − 1), and its inverse, to the few f32 roundings each takes: α 2, β 2.5
    /// gives 4·√(2^0.4 − 1) = 2.2610.
    #[test]
    fn alpha_beta_fwhm_conversion() {
        for (alpha, beta) in [(2.0f32, 2.5f32), (1.0, 1.5), (3.0, 4.0), (6.0, 6.0)] {
            let exact = 2.0 * f64::from(alpha) * (2f64.powf(1.0 / f64::from(beta)) - 1.0).sqrt();
            let fwhm = alpha_beta_to_fwhm(alpha, beta);
            assert!(
                (f64::from(fwhm) - exact).abs() <= 8.0 * f64::from(f32::EPSILON) * exact,
                "α {alpha} β {beta}: {fwhm} vs {exact}"
            );
            let back = fwhm_beta_to_alpha(fwhm, beta);
            assert!(
                (back - alpha).abs() <= 16.0 * f32::EPSILON * alpha,
                "α {alpha} β {beta}: back to {back}"
            );
        }
        assert!((f64::from(alpha_beta_to_fwhm(2.0, 2.5)) - 2.2610).abs() < 1e-4);
    }
}
