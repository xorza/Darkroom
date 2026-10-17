//! [`ProfileBounds`]: the widths a profile fit may take on a stamp, the same for every model.

use std::ops::RangeInclusive;

use crate::math::fwhm;
use crate::math::fwhm::FWHM_PER_SIGMA;

/// The narrowest principal σ a profile fit may take, in px: a Gaussian's, and for a Moffat, the
/// σ of a Gaussian of its FWHM, 1.18 px.
pub(super) const MIN_PROFILE_SIGMA: f64 = 0.5;

/// The FWHM range a profile fit may take on a stamp, stated once so the Gaussian and the Moffat
/// are held to the same widths: from that of a Gaussian of [`MIN_PROFILE_SIGMA`] to that of a
/// Gaussian whose σ is the stamp radius. Each model reads it in its own parameter — a σ, or an α at
/// its β, whose FWHM per unit `2·√(2^(1/β) − 1)` falls below 1 past β ≈ 3, so an α capped at the
/// radius would refuse stars of the expected width.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ProfileBounds {
    min_fwhm: f64,
    max_fwhm: f64,
}

impl ProfileBounds {
    /// The bounds on a stamp of `radius` px.
    pub(super) fn of_stamp(radius: f64) -> Self {
        Self {
            min_fwhm: FWHM_PER_SIGMA * MIN_PROFILE_SIGMA,
            max_fwhm: FWHM_PER_SIGMA * radius,
        }
    }

    /// The bounds as a Gaussian σ.
    pub(super) fn sigma(self) -> RangeInclusive<f64> {
        self.min_fwhm / FWHM_PER_SIGMA..=self.max_fwhm / FWHM_PER_SIGMA
    }

    /// The bounds as a Moffat α at `beta`.
    pub(super) fn alpha(self, beta: f64) -> RangeInclusive<f64> {
        fwhm::to_alpha(self.min_fwhm, beta)..=fwhm::to_alpha(self.max_fwhm, beta)
    }
}

#[cfg(test)]
mod tests {
    use crate::math::fwhm;
    use crate::star_detection::centroid::profile_bounds::ProfileBounds;

    /// On a stamp of radius 7 a Gaussian spans σ 0.5 to 7, and a Moffat the α of the same FWHM at
    /// its β: at β 10, whose FWHM per α is 0.537, α 2.19 to 30.7, where a cap at the radius would
    /// refuse a star of FWHM 4 (α 7.45).
    #[test]
    fn both_models_are_held_to_one_width_range() {
        let bounds = ProfileBounds::of_stamp(7.0);
        let sigma = bounds.sigma();
        assert!((sigma.start() - 0.5).abs() <= f64::EPSILON);
        assert!((sigma.end() - 7.0).abs() <= 8.0 * f64::EPSILON);
        let alpha = bounds.alpha(10.0);
        let at_fwhm_4 = fwhm::to_alpha(4.0, 10.0);
        assert!(
            at_fwhm_4 > 7.0 && alpha.contains(&at_fwhm_4),
            "{alpha:?} {at_fwhm_4}"
        );
        for (end, sigma) in [(*alpha.start(), 0.5), (*alpha.end(), 7.0)] {
            let width = fwhm::from_alpha_beta(end, 10.0);
            assert!(
                (width - sigma * fwhm::FWHM_PER_SIGMA).abs() <= 1e-12,
                "{width}"
            );
        }
    }
}
