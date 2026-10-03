use crate::math::statistics::CHI2_99_2DOF;
use crate::stacking::registration::config::Config;
use crate::stacking::registration::tuning::{
    AUTO_UPGRADE_THRESHOLD, max_sigma_from_fwhm, recovery_radius,
};

/// The scales have to keep their documented relationship to the FWHM, since every threshold
/// downstream is quoted in those terms.
#[test]
fn sigma_and_recovery_radius_track_the_psf_width() {
    // Typical ground seeing, FWHM 3 px: σ_max = 1.5 px, recovery radius = √χ²₀.₉₉(2) × 1.5.
    // The expected radius comes from the closed-form quantile, `−2·ln(0.01)`, not from the
    // constant under test.
    let sigma = max_sigma_from_fwhm(3.0);
    assert_eq!(sigma, 1.5);
    let radius = recovery_radius(sigma);
    let expected = (-2.0 * 0.01_f64.ln()).sqrt() * 1.5;
    assert!(
        (radius - expected).abs() < 1e-12,
        "recovery radius {radius} is not √χ²₀.₉₉(2)·σ_max ({expected})"
    );
    // ...which is the ~1.5 FWHM the docs claim: 3.0349 × 1.5 px over a 3 px FWHM.
    assert!(
        (radius / 3.0 - 1.517).abs() < 1e-3,
        "{radius} is not ~1.5 FWHM"
    );

    // Undersampled: the floor holds σ_max at 0.5 px where FWHM/2 would give 0.25.
    assert_eq!(max_sigma_from_fwhm(0.5), 0.5);
    // ...including the degenerate catalog that would otherwise trip RansacEstimator's assert.
    assert_eq!(max_sigma_from_fwhm(0.0), 0.5);

    // Squaring the radius must land on the boundary MAGSAC scores against, or the two gates
    // disagree about the same point: a square root and a square apart, a few ulps.
    let boundary_sq = CHI2_99_2DOF * sigma * sigma;
    assert!((radius * radius - boundary_sq).abs() <= 4.0 * f64::EPSILON * boundary_sq);
}

/// The ladder bar is stricter than the default accuracy gate; if that inverted, `Auto` would
/// accept rungs `register` then rejects.
#[test]
fn the_ladder_bar_is_stricter_than_the_default_accuracy_gate() {
    let default_gate = Config::default().max_rms_error;
    assert!(
        AUTO_UPGRADE_THRESHOLD < default_gate,
        "ladder bar {AUTO_UPGRADE_THRESHOLD} must be stricter than the {default_gate} gate"
    );
}
