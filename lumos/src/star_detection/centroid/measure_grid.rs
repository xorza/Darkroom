//! [`MeasureGrid`]: what follows from the FWHM a detection expects, shared by every star of it.

use crate::math::fwhm::fwhm_to_sigma;
use crate::star_detection::centroid::stamp::StampGrid;

/// Stamp radius as a multiple of FWHM: 1.75 × FWHM holds about 99% of a Gaussian's flux.
const STAMP_RADIUS_FWHM_FACTOR: f32 = 1.75;

/// The smallest stamp radius, in pixels: enough pixels to fit a profile to a narrow star.
pub(super) const MIN_STAMP_RADIUS: usize = 4;

/// The largest stamp radius, in pixels.
pub(super) const MAX_STAMP_RADIUS: usize = 15;

/// The share of a Moffat profile's flux the sky annulus keeps inside its inner radius.
const ANNULUS_ENCLOSED_FLUX: f64 = 0.99;

/// The Moffat β the annulus is placed for: atmospheric seeing's wings, the heaviest the measurement
/// assumes.
const ANNULUS_MOFFAT_BETA: f64 = 2.5;

/// What one detection's stars are measured with, all from the FWHM it expects.
#[derive(Debug)]
pub(crate) struct MeasureGrid {
    pub(crate) stamp: StampGrid,
    /// σ of the centroid's Gaussian window: the expected star's own, held to `[1, r/2]` so the
    /// window spans at least a pixel and the stamp at least two of its σ.
    pub(super) window_sigma: f64,
    /// The sky annulus: from where a β = 2.5 Moffat of the expected FWHM holds 99% of its flux, so
    /// its wings lift the sky by under half a percent of the flux, out by a stamp radius.
    pub(super) annulus: AnnulusRadii,
}

/// The inner and outer radius of a sky annulus, in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct AnnulusRadii {
    pub(super) inner: usize,
    pub(super) outer: usize,
}

impl MeasureGrid {
    pub(crate) fn new(expected_fwhm: f32) -> Self {
        let radius = Self::stamp_radius(expected_fwhm);
        let window_sigma = f64::from(fwhm_to_sigma(expected_fwhm)).clamp(1.0, radius as f64 / 2.0);
        Self {
            stamp: StampGrid::new(radius),
            window_sigma,
            annulus: AnnulusRadii::enclosing(f64::from(expected_fwhm), radius),
        }
    }

    /// The stamp radius for `expected_fwhm`, held to `[MIN_STAMP_RADIUS, MAX_STAMP_RADIUS]`.
    #[expect(
        clippy::cast_sign_loss,
        reason = "a non-positive FWHM saturates to 0 and takes the minimum radius"
    )]
    pub(super) const fn stamp_radius(expected_fwhm: f32) -> usize {
        let radius = (expected_fwhm * STAMP_RADIUS_FWHM_FACTOR).ceil() as usize;
        if radius < MIN_STAMP_RADIUS {
            MIN_STAMP_RADIUS
        } else if radius > MAX_STAMP_RADIUS {
            MAX_STAMP_RADIUS
        } else {
            radius
        }
    }
}

impl AnnulusRadii {
    /// The annulus for a star of `fwhm` measured on a stamp of `stamp_radius`.
    ///
    /// A Moffat profile of core width α holds `1 − (1 + r²/α²)^(1 − β)` of its flux within `r`,
    /// and its FWHM is `2α·√(2^(1/β) − 1)`; the radius holding a share `f` is then
    /// `FWHM·√((1 − f)^(1/(1 − β)) − 1) / (2·√(2^(1/β) − 1))`, 4.01 FWHM for 99% at β = 2.5. The
    /// inner radius is at least past the stamp.
    #[expect(
        clippy::cast_sign_loss,
        reason = "the radius is a non-negative FWHM times a positive factor"
    )]
    fn enclosing(fwhm: f64, stamp_radius: usize) -> Self {
        let beta = ANNULUS_MOFFAT_BETA;
        let core = (1.0 - ANNULUS_ENCLOSED_FLUX).powf(1.0 / (1.0 - beta)) - 1.0;
        let width = 2.0 * ((2.0f64).powf(1.0 / beta) - 1.0).sqrt();
        let radius = (fwhm * core.sqrt() / width).ceil() as usize;
        let inner = radius.max(stamp_radius + 1);
        Self {
            inner,
            outer: inner + stamp_radius,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::star_detection::centroid::measure_grid::{AnnulusRadii, MeasureGrid};

    /// FWHM 4: a stamp of ⌈7⌉ = 7, a window of σ 4/2.3548 = 1.699, an annulus from ⌈16.04⌉ = 17
    /// out by 7 to 24. FWHM 1: the smallest stamp, 4, a window held to 1, and an annulus from
    /// past the stamp, 5, to 9. FWHM 20: the largest stamp, 15, a window held to 7.5.
    #[test]
    fn the_grid_follows_the_expected_fwhm() {
        let grid = MeasureGrid::new(4.0);
        assert_eq!(grid.stamp.radius, 7);
        assert!(
            (grid.window_sigma - 1.698_643).abs() < 1e-5,
            "{}",
            grid.window_sigma
        );
        assert_eq!(
            grid.annulus,
            AnnulusRadii {
                inner: 17,
                outer: 24
            }
        );
        let narrow = MeasureGrid::new(1.0);
        assert_eq!(narrow.stamp.radius, 4);
        assert_eq!(narrow.window_sigma, 1.0);
        assert_eq!(narrow.annulus, AnnulusRadii { inner: 5, outer: 9 });
        let wide = MeasureGrid::new(20.0);
        assert_eq!(wide.stamp.radius, 15);
        assert_eq!(wide.window_sigma, 7.5);
    }
}
