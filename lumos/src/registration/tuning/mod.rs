//! The pixel scales registration judges residuals against, and where each number comes from.
//!
//! Two quantities decide whether two stars could be the same star, and both scale with the PSF:
//!
//! - [`max_sigma_from_fwhm`] sets `σ_max`, the noise scale the RANSAC scorer takes.
//! - [`recovery_radius`] turns `σ_max` into the distance the final fit's first pass accepts a
//!   nearest neighbour at, at the same 99% confidence the RANSAC scorer's outlier boundary takes.

use crate::math::statistics::CHI2_99_2DOF;

/// `σ_max` (px) for RANSAC scoring, from the median FWHM of the two star catalogs.
///
/// Half the FWHM is not a centroid-noise estimate — real centroid error is
/// `≈ FWHM / (2.355·SNR)`, one to two orders of magnitude smaller. It is an upper bound, and the
/// quantity that makes it the right one is what it implies downstream: the RANSAC scorer treats
/// residuals past `√χ²₀.₉₉(2)·σ_max ≈ 3.03·σ_max` as outliers, so `σ_max = FWHM/2` puts that
/// boundary at **≈ 1.5 FWHM** — a star displaced by more than one and a half PSF widths is a
/// different star, not a mis-centroided one. An upper bound rather than an estimate is what the
/// scorer wants: its loss saturates at that scale, so an over-tight bound discards real matches
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
/// The same 1%-tail test the RANSAC scorer applies, in distance rather than squared distance:
/// `√χ²₀.₉₉(2)·σ_max`. Sharing [`CHI2_99_2DOF`] is the point — recovery admitting matches the
/// RANSAC scorer would call outliers, or refusing ones it accepts, is a contradiction, and two
/// independent literals of the same constant drift apart.
pub(super) fn recovery_radius(max_sigma: f64) -> f64 {
    CHI2_99_2DOF.sqrt() * max_sigma
}

#[cfg(test)]
mod tests;
