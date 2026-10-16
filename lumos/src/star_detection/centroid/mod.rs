//! Sub-pixel centroid computation and star quality metrics.
//!
//! Every star's centre starts from the converged windowed centroid, which a Gaussian or Moffat
//! profile fit may then refine; both report the position's standard error.
//!
//! Positions are f64 end to end. Every accumulator here is already f64, [`Star::pos`] is a
//! [`DVec2`], and registration solves its transforms in f64 — an f32 carrier would only add a
//! narrowing in the middle, and would quantize coarser than the centroid's tolerance beyond
//! x ≈ 1024.

mod covariance;
mod gaussian_fit;
pub(super) mod measure_grid;
mod moffat_fit;
mod profile_bounds;
mod profile_fit;
mod profile_problem;
mod simd;
mod stamp;
mod stamp_background;
mod stamp_metrics;
mod star_noise;
mod windowed_centroid;
mod windowed_stamp;

use glam::DVec2;
use imaginarium::Buffer2;

use crate::bit_buffer2::BitBuffer2;
use crate::image::masked_plane::MaskedPlane;
use crate::star_detection::background::sky_noise::SkyNoise;
use crate::star_detection::centroid::covariance::Cov2;
use crate::star_detection::centroid::gaussian_fit::GaussianFit;
use crate::star_detection::centroid::measure_grid::MeasureGrid;
use crate::star_detection::centroid::moffat_fit::MoffatFit;
use crate::star_detection::centroid::profile_fit::{FitShape, ProfileFit};
use crate::star_detection::centroid::stamp::{FitInputs, StampAt};
use crate::star_detection::centroid::stamp_background::StampBackground;
use crate::star_detection::centroid::stamp_metrics::{StampMetrics, StarBackground};
use crate::star_detection::centroid::star_noise::StarNoise;
use crate::star_detection::centroid::windowed_centroid::{WindowedCentroid, WindowedInputs};
use crate::star_detection::deblend::region::Region;
use crate::star_detection::star::Star;
use crate::star_detection::star_detection_config::measurement_config::{
    CentroidMethod, LocalBackgroundMethod, MeasurementConfig,
};

/// The frame planes a star is measured on: the residual, its sky noise, and which pixels are
/// saturated and which hold no measurement.
#[derive(Debug, Clone, Copy)]
pub(super) struct MeasurePlanes<'a> {
    pub(super) residual: &'a Buffer2<f32>,
    pub(super) sky: &'a SkyNoise,
    pub(super) saturation: &'a BitBuffer2,
    pub(super) no_data: Option<&'a BitBuffer2>,
}

impl MeasurePlanes<'_> {
    /// Measure a star candidate: compute sub-pixel position and quality metrics.
    ///
    /// This is the main entry point for the measurement stage. It takes a detected region and:
    /// 1. measures the local sky the residual still carries, from the annulus in `LocalAnnulus`
    ///    mode;
    /// 2. finds the converged windowed centroid above it ([`WindowedCentroid`]);
    /// 3. with a profile fit configured, fits from there, stamped again once at the fit's centre
    ///    when it moved more than half the stamp radius; a fit that fails leaves the windowed
    ///    centroid;
    /// 4. computes flux, SNR, sharpness and roundness at the centre ([`StampMetrics`]), and the
    ///    FWHM and eccentricity the fit does not give from the windowed moments.
    ///
    /// Every star leaves with a position σ: the fit's, from `(JᵀWJ)⁻¹·χ²/(n − p)`, or the windowed
    /// centroid's, from the pixel noise.
    ///
    /// Returns `None` if the candidate fails quality checks during measurement.
    pub(super) fn measure_star(
        self,
        region: &Region,
        config: &MeasurementConfig,
        grid: &MeasureGrid,
    ) -> Option<Star> {
        let MeasurePlanes {
            residual,
            sky,
            saturation,
            no_data,
        } = self;
        let stamp_radius = grid.stamp.radius;
        let stamp_at = |pos: DVec2| StampAt {
            residual,
            pos,
            radius: stamp_radius,
        };
        let start = DVec2::new(region.peak.x as f64, region.peak.y as f64);
        // A stamp over a pixel with no measurement reads its fill as one, so such a star is not
        // measured: at its peak, and again at the centre it moves to.
        let holds_no_data = |at: DVec2| {
            no_data.is_some_and(|no_data| {
                stamp_at(at)
                    .centre()
                    .is_some_and(|centre| no_data.any_in_square(centre, stamp_radius))
            })
        };
        if holds_no_data(start) {
            return None;
        }

        // None in GlobalMap mode, or when fewer than 10 annulus pixels lie in the frame. A missing
        // annulus leaves the map's noise at the star, held to the frame's floor, for the centroid
        // and the fit, but must NOT become a metrics override: the map's noise at one pixel stands
        // in for the stamp only where nothing better was measured.
        let annulus_at = |at: DVec2| match config.local_background {
            LocalBackgroundMethod::GlobalMap => None,
            LocalBackgroundMethod::LocalAnnulus => StampBackground::measure(
                MaskedPlane {
                    pixels: residual,
                    mask: no_data,
                },
                at,
                grid.annulus,
            ),
        };
        let start_annulus = annulus_at(start);
        let (local_offset, local_noise) = start_annulus
            .map_or((0.0, sky.noise[(region.peak.x, region.peak.y)]), |local| {
                (local.offset, local.noise)
            });
        let noise = StarNoise {
            background_sigma: f64::from(local_noise.max(sky.floor)),
            electrons_per_unit: config.electrons_per_unit.map(f64::from),
        };

        let windowed = WindowedCentroid::measure(
            residual,
            grid,
            WindowedInputs {
                start,
                offset: local_offset,
                noise,
            },
        )?;

        // Weighted by the CCD equation only when the star's own photons are known; with the
        // background's noise alone every pixel weighs the same.
        let fit_noise = noise.electrons_per_unit.is_some().then_some(noise);
        let fit_inputs = |at: DVec2| FitInputs {
            pixels: residual,
            pos: at,
            grid: &grid.stamp,
            background: local_offset,
            noise: fit_noise,
        };
        let fit = match config.centroid_method {
            CentroidMethod::WeightedMoments => None,
            CentroidMethod::GaussianFit => ProfileFit::restamped(
                |at| GaussianFit::new(fit_inputs(at)).map(ProfileFit::from),
                windowed.pos,
                stamp_radius,
            ),
            CentroidMethod::MoffatFit { beta } => ProfileFit::restamped(
                |at| MoffatFit::new(fit_inputs(at), beta).map(ProfileFit::from),
                windowed.pos,
                stamp_radius,
            ),
        };
        let (pos, position_sigma) = fit.map_or((windowed.pos, windowed.sigma), |fit| {
            (fit.pos, fit.position_sigma)
        });

        if holds_no_data(pos) {
            return None;
        }
        // `StampBackground::measure` samples by rounded centre, so the annulus measured at the
        // start stands unless the centre moved to another pixel.
        let annulus_background = if pos.round() == start.round() {
            start_annulus
        } else {
            annulus_at(pos)
        };

        // Flux, SNR, sharpness and roundness come from the stamp whatever the method; where the
        // annulus measured the sky, against its offset, its noise and its sample count.
        let metrics = StampMetrics::measure(
            stamp_at(pos),
            grid.window_sigma,
            StarBackground {
                offset: annulus_background.map_or(0.0, |local| local.offset),
                noise: annulus_background.map_or(noise, |local| StarNoise {
                    background_sigma: f64::from(local.noise.max(sky.floor)),
                    ..noise
                }),
                sky_samples: annulus_background.map(|local| local.samples),
            },
        )?;
        // A fit's widths replace the moments'.
        let (fwhm, eccentricity) = match fit.map(|fit| fit.shape) {
            Some(FitShape::Elliptical(covariance)) => {
                (covariance.fwhm(), covariance.eccentricity())
            }
            Some(FitShape::Round { fwhm }) => {
                (fwhm, metrics.moment_shape().map_or(0.0, Cov2::eccentricity))
            }
            None => {
                let shape = metrics.moment_shape();
                (
                    shape.map_or(0.0, Cov2::fwhm),
                    shape.map_or(0.0, Cov2::eccentricity),
                )
            }
        };

        Some(Star {
            pos,
            position_sigma,
            flux: metrics.flux,
            fwhm,
            eccentricity,
            snr: metrics.snr,
            peak: metrics.peak,
            saturated: saturation.get_at(region.peak),
            sharpness: metrics.sharpness,
            roundness: metrics.roundness,
        })
    }
}

#[cfg(feature = "internals")]
pub(crate) mod bench;
#[cfg(test)]
mod tests;
