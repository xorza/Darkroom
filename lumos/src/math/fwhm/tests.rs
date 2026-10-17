use std::f64::consts::LN_2;

use crate::math::fwhm;
use crate::math::fwhm::{FWHM_PER_SIGMA, FWHM_PER_SIGMA_F32};

#[test]
fn the_constant_is_two_root_two_ln_two() {
    assert_eq!(FWHM_PER_SIGMA, 2.0 * (2.0 * LN_2).sqrt());
    assert_eq!(fwhm::from_sigma(1.0), FWHM_PER_SIGMA_F32);
    assert_eq!(fwhm::to_sigma(FWHM_PER_SIGMA_F32), 1.0);
}

/// A round Gaussian of σ 2 has `det = 2⁴`, and the FWHM is `2·2.3548`; one of σ 1 by 4 has the
/// same area, and so the same FWHM. Exact powers of two through two square roots, then the
/// f32 constant.
#[test]
fn equal_area_fwhm_is_the_geometric_mean_width() {
    assert_eq!(fwhm::equal_area(16.0), 2.0 * FWHM_PER_SIGMA_F32);
    assert_eq!(fwhm::equal_area(1.0 * 16.0), fwhm::equal_area(4.0 * 4.0));
    assert_eq!(fwhm::equal_area(-1.0), 0.0);
}

/// 4.5 → 4.5 / 2.35482 → back: a division and a multiplication by one f32 constant, each
/// within half an ulp, so the round trip lands within one ulp of 4.5 (4.8e-7).
#[test]
fn fwhm_sigma_conversion_roundtrip() {
    let back = fwhm::from_sigma(fwhm::to_sigma(4.5));
    assert!((back - 4.5).abs() <= 4.5 * f32::EPSILON, "{back}");
}

/// FWHM = 2α·√(2^(1/β) − 1), and its inverse, in f64 to a few ulps: α 2, β 2.5 gives
/// 4·√(2^0.4 − 1) = 2.2610. At β = 1 the factor is 2, the Lorentzian's.
#[test]
fn alpha_beta_fwhm_conversion() {
    assert_eq!(fwhm::per_alpha(1.0), 2.0);
    for (alpha, beta) in [(2.0, 2.5), (1.0, 1.5), (3.0, 4.0), (6.0, 6.0)] {
        let exact = 2.0 * alpha * (2f64.powf(1.0 / beta) - 1.0).sqrt();
        let fwhm = fwhm::from_alpha_beta(alpha, beta);
        assert!(
            (fwhm - exact).abs() <= 4.0 * f64::EPSILON * exact,
            "α {alpha} β {beta}: {fwhm} vs {exact}"
        );
        let back = fwhm::to_alpha(fwhm, beta);
        assert!(
            (back - alpha).abs() <= 4.0 * f64::EPSILON * alpha,
            "α {alpha} β {beta}: back to {back}"
        );
    }
    assert!((fwhm::from_alpha_beta(2.0, 2.5) - 2.2610).abs() < 1e-4);
}
