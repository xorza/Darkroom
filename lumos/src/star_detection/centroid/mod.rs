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
mod lm_optimizer;
mod local_background;
pub(crate) mod measure_grid;
mod moffat_fit;
mod simd;
pub(crate) mod stamp;
mod star_noise;
mod windowed_centroid;

use glam::DVec2;

use crate::bit_buffer2::BitBuffer2;
use crate::math::lm_controller::LmFit;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;
use crate::star_detection::background::sky_noise::SkyNoise;
use crate::star_detection::centroid::covariance::{Cov2, MIN_SIGMA_SQ};
use crate::star_detection::centroid::local_background::LocalBackground;
use crate::star_detection::centroid::measure_grid::{MAX_STAMP_RADIUS, MeasureGrid};
use crate::star_detection::centroid::star_noise::StarNoise;
use crate::star_detection::centroid::windowed_centroid::{WindowedCentroid, WindowedInputs};
use crate::star_detection::config::measurement_config::{
    CentroidMethod, LocalBackgroundMethod, MeasurementConfig,
};
use crate::star_detection::deblend::region::Region;
use crate::star_detection::roundness::{Roundness, RoundnessStamp};
use crate::star_detection::star::Star;
use gaussian_fit::GaussianFit;
use imaginarium::Buffer2;
use moffat_fit::MoffatFit;

/// Maximum stamp side length in pixels (31 for `stamp_radius=15`).
pub(super) const MAX_STAMP_SIZE: usize = 2 * MAX_STAMP_RADIUS + 1;

/// Maximum stamp pixels (31×31 for `stamp_radius=15`).
pub(super) const MAX_STAMP_PIXELS: usize = MAX_STAMP_SIZE.pow(2);

/// The pixel nearest `pos`, when a stamp of `stamp_radius` around it lies wholly inside `size`.
///
/// Compared in f64, where `x + r < width` cannot underflow as `x < width − r` would in usize, and a
/// NaN fails every comparison.
#[inline]
#[expect(
    clippy::cast_sign_loss,
    reason = "both coordinates are checked to be at least the radius before the cast"
)]
pub(super) fn stamp_centre(pos: DVec2, size: Size2us, stamp_radius: usize) -> Option<Vec2us> {
    let (x, y) = (pos.x.round(), pos.y.round());
    let radius = stamp_radius as f64;
    let inside = x >= radius
        && y >= radius
        && x + radius < size.width as f64
        && y + radius < size.height as f64;
    inside.then(|| Vec2us::new(x as usize, y as usize))
}

/// Whether a profile fit's centre landed somewhere its caller can use: finite, and within
/// `stamp_radius` of where the fit started. The shape is each model's own to judge, against the
/// bounds its `constrain` holds it to.
///
/// The centre needs [`DVec2::is_finite`] of its own: `max_element` reduces with [`f64::max`], which
/// *ignores* NaN and returns the other lane, so a NaN x-coordinate would silently compare as the
/// (finite) y-offset.
///
/// A rejected fit is not an error — [`measure_star`] falls back to the moment-based centroid.
fn fit_is_plausible(result_pos: DVec2, input_pos: DVec2, stamp_radius: usize) -> bool {
    result_pos.is_finite() && (result_pos - input_pos).abs().max_element() <= stamp_radius as f64
}

/// The position σ of a profile fit whose first two parameters are the centre: `(JᵀWJ)⁻¹·χ²/(n − p)`
/// per axis, which holds when the weights are right up to a common scale, and `√((σ_x² + σ_y²)/2)`
/// of them. `None` when the Hessian at the solution is singular, or the stamp holds no more
/// samples than parameters.
fn position_sigma<const N: usize>(fit: &LmFit<N>, samples: usize) -> Option<f64> {
    let inverse = fit.inverse_hessian_diagonal?;
    let freedom = samples.checked_sub(N).filter(|&freedom| freedom > 0)?;
    let scale = fit.chi2 / freedom as f64;
    Some(((inverse[0] + inverse[1]) * scale / 2.0).sqrt())
}

/// Measure a star candidate: compute sub-pixel position and quality metrics.
///
/// This is the main entry point for the measurement stage. It takes a detected region and:
/// 1. measures the local sky the residual still carries, from the annulus in `LocalAnnulus` mode;
/// 2. finds the converged windowed centroid above it ([`WindowedCentroid`]);
/// 3. with a profile fit configured, fits from there, stamped again once at the fit's centre when
///    it moved more than half the stamp radius; a fit that fails leaves the windowed centroid;
/// 4. computes flux, FWHM, eccentricity, SNR, sharpness and roundness at the centre.
///
/// Every star leaves with a position σ: the fit's, from `(JᵀWJ)⁻¹·χ²/(n − p)`, or the windowed
/// centroid's, from the pixel noise.
///
/// Returns `None` if the candidate fails quality checks during measurement.
pub(super) fn measure_star(
    residual: &Buffer2<f32>,
    sky: &SkyNoise,
    saturation: &BitBuffer2,
    region: &Region,
    config: &MeasurementConfig,
    grid: &MeasureGrid,
) -> Option<Star> {
    let stamp_radius = grid.stamp.radius;
    let start = DVec2::new(region.peak.x as f64, region.peak.y as f64);

    // None in GlobalMap mode, or when fewer than 10 annulus pixels lie in the frame. A missing
    // annulus leaves the map's noise at the star, held to the frame's floor, for the centroid and
    // the fit, but must NOT become a metrics override: the map's noise at one pixel stands in for
    // the stamp only where nothing better was measured.
    let annulus_at = |at: DVec2| match config.local_background {
        LocalBackgroundMethod::GlobalMap => None,
        LocalBackgroundMethod::LocalAnnulus => LocalBackground::measure(residual, at, grid.annulus),
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
        start,
        grid,
        WindowedInputs {
            offset: local_offset,
            noise,
        },
    )?;
    let mut pos = windowed.pos;
    let mut position_sigma = windowed.sigma;

    // A converged fit's widths replace the moment-based FWHM and eccentricity.
    let mut fit_fwhm: Option<f32> = None;
    let mut fit_eccentricity: Option<f32> = None;
    // Weighted by the CCD equation only when the star's own photons are known; with the
    // background's noise alone every pixel weighs the same.
    let fit_noise = noise.electrons_per_unit.is_some().then_some(noise);
    match config.centroid_method {
        CentroidMethod::GaussianFit => {
            let fit = restamped(
                |at| GaussianFit::new(residual, at, &grid.stamp, local_offset, fit_noise),
                |fit| fit.pos,
                windowed.pos,
                stamp_radius,
            );
            if let Some(fit) = fit {
                pos = fit.pos;
                position_sigma = fit.position_sigma;
                fit_fwhm = Some(fit.covariance.fwhm());
                fit_eccentricity = Some(fit.covariance.eccentricity());
            }
        }
        CentroidMethod::MoffatFit { beta } => {
            let fit = restamped(
                |at| MoffatFit::new(residual, at, &grid.stamp, local_offset, fit_noise, beta),
                |fit| fit.pos,
                windowed.pos,
                stamp_radius,
            );
            if let Some(fit) = fit {
                pos = fit.pos;
                position_sigma = fit.position_sigma;
                fit_fwhm = Some(fit.fwhm);
                // Moffat is radially symmetric (single alpha) — eccentricity stays moment-based
            }
        }
        CentroidMethod::WeightedMoments => {}
    }

    // `compute_annulus_background` samples by rounded centre, so the annulus measured at the start
    // stands unless the centre moved to another pixel.
    let annulus_background = if pos.round() == start.round() {
        start_annulus
    } else {
        annulus_at(pos)
    };

    // Flux, SNR, sharpness and roundness come from the stamp whatever the method; where the
    // annulus measured the sky, against its offset, its noise and its sample count.
    let mut star = compute_star(
        residual,
        pos,
        stamp_radius,
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
    star.saturated = saturation.get_at(region.peak);
    star.position_sigma = position_sigma;

    if let Some(fwhm) = fit_fwhm {
        star.fwhm = fwhm;
    }
    if let Some(ecc) = fit_eccentricity {
        star.eccentricity = ecc;
    }

    Some(star)
}

/// A profile fit from `start`, fitted again once from its own centre when it moved more than half
/// the stamp radius: a stamp centred far off the star truncates its wings, which pulls the fit.
/// `None` when either fit fails, or the second moves that far again.
fn restamped<T>(
    fit: impl Fn(DVec2) -> Option<T>,
    centre: impl Fn(&T) -> DVec2,
    start: DVec2,
    stamp_radius: usize,
) -> Option<T> {
    let half = stamp_radius as f64 / 2.0;
    let first = fit(start)?;
    let moved = centre(&first);
    if (moved - start).abs().max_element() <= half {
        return Some(first);
    }
    let second = fit(moved)?;
    ((centre(&second) - moved).abs().max_element() <= half).then_some(second)
}

/// How closely a profile fit integrates its model over each pixel, relative to the amplitude:
/// half an ulp of the f32 samples it is fit to.
const PIXEL_MEAN_TOLERANCE: f64 = 1.0 / (1u64 << 25) as f64;

/// The narrowest principal σ a profile fit may take, in px: a Gaussian's, and for a Moffat, the
/// σ of a Gaussian of its FWHM, 1.18 px.
const MIN_PROFILE_SIGMA: f64 = 0.5;

/// What a star's metrics are measured against: the local sky the residual still carries, the noise
/// of the star's pixels, and how many samples the sky was measured from, when an annulus measured
/// it.
#[derive(Debug, Clone, Copy)]
struct StarBackground {
    offset: f32,
    noise: StarNoise,
    sky_samples: Option<usize>,
}

/// Construct a star and compute its quality metrics at the given position, against `background`,
/// over a stamp of `stamp_radius`: flux, the star's own peak above the sky, FWHM and eccentricity
/// from the windowed covariance, the SNR by the CCD equation, the sharpness of its own peak, and
/// the DAOFIND roundness against a PSF of `psf_sigma`. `None` when the stamp leaves the frame or
/// holds no star — no net or no positive light, a non-finite flux or SNR, or a marginal with no
/// height — and the position σ is the caller's to set.
fn compute_star(
    residual: &Buffer2<f32>,
    pos: DVec2,
    stamp_radius: usize,
    psf_sigma: f64,
    background: StarBackground,
) -> Option<Star> {
    let offset = background.offset;
    let centre = stamp_centre(
        pos,
        Size2us::new(residual.width(), residual.height()),
        stamp_radius,
    )?;

    // Flux and core flux sum the *signed* residual: sky noise is zero-mean, and clipping each pixel
    // at zero would turn it into a positive bias of about 0.4σ per pixel. The second moments are
    // weights, and a weight cannot be negative, so they take the clipped residual and normalize by
    // its own sum.
    let mut flux = 0.0f64;
    let mut weight_sum = 0.0f64;
    let mut core_flux = 0.0f64;
    let mut sum_x2 = 0.0f64;
    let mut sum_y2 = 0.0f64;
    let mut sum_xy = 0.0f64;
    let stamp_size = 2 * stamp_radius + 1;
    let mut stamp = [0.0f64; MAX_STAMP_PIXELS];

    for (my, y) in (centre.y - stamp_radius..=centre.y + stamp_radius).enumerate() {
        let px_row = residual.row(y);
        let ady = my.abs_diff(stamp_radius);
        for (mx, x) in (centre.x - stamp_radius..=centre.x + stamp_radius).enumerate() {
            let signal = f64::from(px_row[x] - offset);
            stamp[my * stamp_size + mx] = signal;
            let value = signal.max(0.0);
            flux += signal;
            weight_sum += value;
            if mx.abs_diff(stamp_radius) <= 1 && ady <= 1 {
                core_flux += signal;
            }
            let fx = x as f64 - pos.x;
            let fy = y as f64 - pos.y;
            sum_x2 += value * fx * fx;
            sum_y2 += value * fy * fy;
            sum_xy += value * fx * fy;
        }
    }

    // No net signal above the sky, or nothing positive to weight the moments with: not a star.
    if flux < f64::EPSILON || weight_sum < f64::EPSILON {
        return None;
    }

    // Adaptive windowed second moments: Gaussian-weight by an iteratively-matched window to
    // suppress wing noise, then deconvolve the window so FWHM/eccentricity stay unbiased. Seed the
    // window from the plain moment; fall back to the plain moments if it can't converge to a
    // valid (positive-definite) covariance.
    let seed_sigma_sq = ((sum_x2 + sum_y2) / weight_sum / 2.0).max(MIN_SIGMA_SQ);
    let cov = Cov2::windowed(residual, offset, pos, stamp_radius, seed_sigma_sq).unwrap_or(Cov2 {
        xx: sum_x2 / weight_sum,
        yy: sum_y2 / weight_sum,
        xy: sum_xy / weight_sum,
    });
    // The PSF's own shape, as the fits report it. A source the moments place inside a pixel is
    // unresolved: FWHM 0, and no elongation to measure.
    let shape = cov.less_pixel();

    let npix = stamp_size * stamp_size;
    let snr = background.noise.snr(flux, npix, background.sky_samples);
    if !flux.is_finite() || !snr.is_finite() {
        return None;
    }
    // The star's own peak, at its centre pixel: the brightest pixel of the stamp may be a
    // companion's, and its sharpness then reads 1.
    let peak = stamp[stamp_radius * stamp_size + stamp_radius];
    let sharpness = if core_flux > 0.0 {
        (peak / core_flux).clamp(0.0, 1.0) as f32
    } else {
        1.0
    };
    let roundness = Roundness::measure(RoundnessStamp {
        values: &stamp[..npix],
        radius: stamp_radius,
        psf_sigma,
    })?;

    Some(Star {
        pos,
        // The caller's, from the centroid or the fit that found `pos`.
        position_sigma: f64::NAN,
        flux: flux as f32,
        fwhm: shape.map_or(0.0, Cov2::fwhm),
        eccentricity: shape.map_or(0.0, Cov2::eccentricity),
        snr: snr as f32,
        peak: peak as f32,
        saturated: false,
        sharpness,
        roundness,
    })
}

#[cfg(all(test, feature = "bench"))]
mod bench;
#[cfg(test)]
mod tests;
