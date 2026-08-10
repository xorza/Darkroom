//! The pixel scales registration judges residuals against, and where each number comes from.
//!
//! Three quantities decide whether two stars are the same star and whether a model fits, and they
//! were previously three bare literals in `mod.rs` — two of which are `0.5` and mean entirely
//! different things. They are collected here because they are only meaningful relative to each
//! other:
//!
//! - [`max_sigma_from_fwhm`] sets σ_max, the noise scale MAGSAC scores against. **Seeing-relative.**
//! - [`recovery_radius`] turns σ_max into the distance match recovery accepts a nearest neighbour
//!   at. **Derived from σ_max**, at the same 99% confidence MAGSAC uses for its outlier boundary.
//! - [`AUTO_UPGRADE_THRESHOLD`] is the RMS a model must reach for `Auto` to stop adding degrees of
//!   freedom. **Absolute**, and deliberately not seeing-relative — see its docs.
//!
//! The first two answer "could these be the same star?", which scales with the PSF. The third
//! answers "is this model already good enough?", which does not. Collapsing them into one tunable
//! would be wrong in both directions.

use crate::math::statistics::CHI2_99_2DOF;

/// σ_max (px) for MAGSAC scoring, from the median FWHM of the two star catalogs.
///
/// Half the FWHM is not a centroid-noise estimate — real centroid error is
/// `≈ FWHM / (2.355·SNR)`, one to two orders of magnitude smaller. It is an upper bound, and the
/// quantity that makes it the right one is what it implies downstream: MAGSAC treats residuals past
/// `√χ²₀.₉₉(2)·σ_max ≈ 3.03·σ_max` as outliers, so `σ_max = FWHM/2` puts that boundary at
/// **≈ 1.5 FWHM** — a star displaced by more than one and a half PSF widths is a different star,
/// not a mis-centroided one. σ_max being an upper bound rather than an estimate is what MAGSAC
/// wants: it integrates the loss over `[0, σ_max]`, so an over-tight bound discards real matches
/// while a loose one only costs discrimination.
///
/// The 0.5 px floor covers undersampled frames: at FWHM < 1 px, half the FWHM would put the
/// recovery radius below the centroid quantization itself. It also guarantees the `σ_max > 0`
/// assert in `RansacEstimator::new`, which a catalog of zero-FWHM stars would otherwise trip.
pub(super) fn max_sigma_from_fwhm(median_fwhm: f64) -> f64 {
    (median_fwhm * 0.5).max(0.5)
}

/// Distance (px) within which match recovery accepts a target star as the partner of a predicted
/// reference position.
///
/// The same 1%-tail test MAGSAC applies, in distance rather than squared distance:
/// `√χ²₀.₉₉(2)·σ_max`. Sharing [`CHI2_99_2DOF`] is the point — recovery admitting matches MAGSAC's
/// scorer would call outliers (or refusing ones it accepts) is a contradiction, and the two
/// literals had already rounded apart.
pub(super) fn recovery_radius(max_sigma: f64) -> f64 {
    CHI2_99_2DOF.sqrt() * max_sigma
}

/// Maximum RMS (px) at which an `Auto` rung is accepted before escalating to a model with more
/// degrees of freedom.
///
/// Absolute, not seeing-relative, because it answers a different question from σ_max: not "is this
/// pair plausible?" but "would another degree of freedom fit anything but noise?". Half a pixel
/// sits above the centroid noise floor of any reasonable frame (`FWHM/(2.355·SNR)` is ~0.2 px even
/// at FWHM 5 px and SNR 10) and well below the residual a genuinely wrong model leaves — the
/// anisotropic and perspective fixtures in `auto_ladder_selects_simplest_adequate_model` leave
/// pixels, not tenths. Scaling it with seeing would do the opposite of what it is for: bad seeing
/// would buy a wrong model a pass.
///
/// It is a ceiling on the *ladder*, not on the result. `Config::max_rms_error` (default 2.0) is the
/// caller's gate and is normally looser; where it is tighter, `auto_ladder` takes the smaller of the
/// two, since accepting a rung the caller will then reject helps nobody.
pub(super) const AUTO_UPGRADE_THRESHOLD: f64 = 0.5;

#[cfg(test)]
mod tests {
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
        assert!(max_sigma_from_fwhm(0.0) > 0.0);

        // Squaring the radius must land exactly on the boundary MAGSAC scores against, or the two
        // gates disagree about the same point.
        let boundary_sq = CHI2_99_2DOF * sigma * sigma;
        assert!((radius * radius - boundary_sq).abs() < 1e-9);
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
}
