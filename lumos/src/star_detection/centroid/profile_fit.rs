//! [`ProfileFit`]: what a converged profile fit gives a star, whatever its model.

use glam::DVec2;

use crate::star_detection::centroid::covariance::Cov2;
use crate::star_detection::centroid::gaussian_fit::GaussianFit;
use crate::star_detection::centroid::moffat_fit::MoffatFit;

/// A converged profile fit: the centre, its σ from `(JᵀWJ)⁻¹·χ²/(n − p)`, and the shape the model
/// can tell.
#[derive(Debug, Clone, Copy)]
pub(super) struct ProfileFit {
    pub(super) pos: DVec2,
    pub(super) position_sigma: f64,
    pub(super) shape: FitShape,
}

/// The shape a profile model fits.
#[derive(Debug, Clone, Copy)]
pub(super) enum FitShape {
    /// The profile's covariance, in px²: an elliptical Gaussian's.
    Elliptical(Cov2),
    /// The FWHM alone: a Moffat's, radially symmetric with one α, so its elongation stays the
    /// moments' to measure.
    Round { fwhm: f32 },
}

impl ProfileFit {
    /// The fit `fit` makes from `start`, fitted again once from its own centre when it moved more
    /// than half the stamp radius: a stamp centred far off the star truncates its wings, which
    /// pulls the fit. `None` when either fit fails, or the second moves that far again.
    pub(super) fn restamped(
        fit: impl Fn(DVec2) -> Option<Self>,
        start: DVec2,
        stamp_radius: usize,
    ) -> Option<Self> {
        let half = stamp_radius as f64 / 2.0;
        let first = fit(start)?;
        if (first.pos - start).abs().max_element() <= half {
            return Some(first);
        }
        let second = fit(first.pos)?;
        ((second.pos - first.pos).abs().max_element() <= half).then_some(second)
    }
}

impl From<GaussianFit> for ProfileFit {
    fn from(fit: GaussianFit) -> Self {
        Self {
            pos: fit.pos,
            position_sigma: fit.position_sigma,
            shape: FitShape::Elliptical(fit.covariance),
        }
    }
}

impl From<MoffatFit> for ProfileFit {
    fn from(fit: MoffatFit) -> Self {
        Self {
            pos: fit.pos,
            position_sigma: fit.position_sigma,
            shape: FitShape::Round { fwhm: fit.fwhm },
        }
    }
}
