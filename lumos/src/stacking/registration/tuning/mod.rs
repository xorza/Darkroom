//! The pixel scales registration judges residuals against, and where each number comes from.
//!
//! Three quantities decide whether two stars are the same star and whether a model fits. They are
//! collected here because they are only meaningful relative to each other, and because two of them
//! are the number `0.5` and mean entirely different things:
//!
//! - [`max_sigma_from_fwhm`] sets `σ_max`, the noise scale MAGSAC scores against. **Seeing-relative.**
//! - [`recovery_radius`] turns `σ_max` into the distance match recovery accepts a nearest neighbour
//!   at. **Derived from `σ_max`**, at the same 99% confidence MAGSAC uses for its outlier boundary.
//! - [`AUTO_UPGRADE_THRESHOLD`] is the RMS a model must reach for `Auto` to stop adding degrees of
//!   freedom. **Absolute**, and deliberately not seeing-relative — see its docs.
//!
//! The first two answer "could these be the same star?", which scales with the PSF. The third
//! answers "is this model already good enough?", which does not. Collapsing them into one tunable
//! would be wrong in both directions.

use crate::math::statistics::CHI2_99_2DOF;

/// `σ_max` (px) for MAGSAC scoring, from the median FWHM of the two star catalogs.
///
/// Half the FWHM is not a centroid-noise estimate — real centroid error is
/// `≈ FWHM / (2.355·SNR)`, one to two orders of magnitude smaller. It is an upper bound, and the
/// quantity that makes it the right one is what it implies downstream: MAGSAC treats residuals past
/// `√χ²₀.₉₉(2)·σ_max ≈ 3.03·σ_max` as outliers, so `σ_max = FWHM/2` puts that boundary at
/// **≈ 1.5 FWHM** — a star displaced by more than one and a half PSF widths is a different star,
/// not a mis-centroided one. `σ_max` being an upper bound rather than an estimate is what MAGSAC
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
/// scorer would call outliers, or refusing ones it accepts, is a contradiction, and two independent
/// literals of the same constant drift apart.
pub(super) fn recovery_radius(max_sigma: f64) -> f64 {
    CHI2_99_2DOF.sqrt() * max_sigma
}

/// Maximum RMS (px) at which an `Auto` rung is accepted before escalating to a model with more
/// degrees of freedom.
///
/// Absolute, not seeing-relative, because it answers a different question from `σ_max`: not "is
/// this pair plausible?" but "would another degree of freedom fit anything but noise?". Half a
/// pixel sits above the centroid noise floor of any reasonable frame (`FWHM/(2.355·SNR)` is ~0.2 px
/// even at FWHM 5 px and SNR 10) and well below the residual a genuinely wrong model leaves — the
/// anisotropic and perspective fixtures in `auto_ladder_selects_simplest_adequate_model` leave
/// pixels, not tenths. Scaling it with seeing would do the opposite of what it is for: bad seeing
/// would buy a wrong model a pass.
///
/// It is a ceiling on the *ladder*, not on the result. `Config::max_rms_error` (default 2.0) is the
/// caller's gate and is normally looser; where it is tighter, `auto_ladder` takes the smaller of
/// the two, since accepting a rung the caller will then reject helps nobody.
pub(super) const AUTO_UPGRADE_THRESHOLD: f64 = 0.5;

#[cfg(test)]
mod tests;
