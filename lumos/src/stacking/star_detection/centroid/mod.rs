//! Sub-pixel centroid computation and star quality metrics.
//!
//! Uses iterative weighted centroid algorithm for sub-pixel accurate positioning,
//! typically achieving ~0.05 pixel accuracy.
//!
//! Also provides 2D Gaussian and Moffat profile fitting for higher precision
//! centroid computation (~0.01 pixel accuracy).
//!
//! Positions are f64 end to end. Every accumulator here is already f64, [`Star::pos`] is a
//! [`DVec2`], and registration solves its transforms in f64 — an f32 carrier would only add a
//! narrowing in the middle. It would also quantize coarser than
//! [`CENTROID_CONVERGENCE_THRESHOLD`] beyond x ≈ 1024, which silently turns the moments loop's
//! convergence test into an exact-equality check on the outer parts of a large frame.

mod covariance;
mod gaussian_fit;
mod lm_optimizer;
mod local_background;
mod moffat_fit;
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
mod simd;
pub(crate) mod stamp;

use glam::DVec2;

use crate::bit_buffer2::BitBuffer2;
use crate::math::fwhm::fwhm_to_sigma;
use crate::math::size2us::Size2us;
use crate::stacking::star_detection::background::sky_noise::SkyNoise;
use crate::stacking::star_detection::centroid::covariance::{
    Cov2, MIN_SIGMA_SQ, windowed_covariance,
};
use crate::stacking::star_detection::centroid::local_background::{
    LocalBackground, compute_annulus_background,
};
use crate::stacking::star_detection::centroid::stamp::{FitNoise, StampGrid};
use crate::stacking::star_detection::config::measurement_config::{
    CentroidMethod, LocalBackgroundMethod, MeasurementConfig, NoiseModel,
};
use crate::stacking::star_detection::deblend::region::Region;
use crate::stacking::star_detection::roundness::Roundness;
use crate::stacking::star_detection::star::Star;
use gaussian_fit::{GaussianFit, GaussianFitConfig};
use imaginarium::Buffer2;
use moffat_fit::{MoffatFit, MoffatFitConfig};

/// Stamp radius as a multiple of FWHM.
///
/// A stamp radius of 1.75 × FWHM captures approximately 99% of the PSF flux
/// for a Gaussian profile, providing accurate centroid and flux measurements
/// while minimizing background contamination.
const STAMP_RADIUS_FWHM_FACTOR: f32 = 1.75;

/// Minimum stamp radius in pixels.
///
/// Ensures sufficient pixels for accurate centroid computation even for
/// very small PSFs or undersampled images.
const MIN_STAMP_RADIUS: usize = 4;

/// Maximum stamp radius in pixels.
///
/// Limits computation time and prevents excessive background inclusion
/// for very large PSFs.
const MAX_STAMP_RADIUS: usize = 15;

/// Maximum stamp side length in pixels (31 for `stamp_radius=15`).
pub(super) const MAX_STAMP_SIZE: usize = 2 * MAX_STAMP_RADIUS + 1;

/// Maximum stamp pixels (31×31 for `stamp_radius=15`).
pub(super) const MAX_STAMP_PIXELS: usize = MAX_STAMP_SIZE.pow(2);

/// Maximum annulus outer radius (1.5 × `MAX_STAMP_RADIUS`, rounded up).
const MAX_ANNULUS_OUTER_RADIUS: usize = (MAX_STAMP_RADIUS * 3).div_ceil(2); // = 23

/// Maximum annulus pixels for `LocalAnnulus` background method.
/// Computed as the area of a square with side `2×outer_radius+1`.
pub(super) const MAX_ANNULUS_PIXELS: usize = (2 * MAX_ANNULUS_OUTER_RADIUS + 1).pow(2); // = 47² = 2209

/// Centroid convergence threshold in pixels.
///
/// Iteration stops when the distance moved is less than this value.
/// Set to 0.0001 (0.1 millipixel) for sub-pixel astrometric precision.
const CENTROID_CONVERGENCE_THRESHOLD: f64 = 0.0001;

/// Maximum weighted-moments iterations for standalone centroid (no fitting follows).
const MAX_MOMENTS_ITERATIONS: usize = 10;

/// Weighted-moments iterations when L-M fitting follows.
/// Only needs to provide a rough seed — L-M refines position independently.
const MOMENTS_ITERATIONS_BEFORE_FIT: usize = 2;

/// Convergence threshold in pixels squared.
const CONVERGENCE_THRESHOLD_SQ: f64 =
    CENTROID_CONVERGENCE_THRESHOLD * CENTROID_CONVERGENCE_THRESHOLD;

/// Compute stamp radius from expected FWHM.
#[inline]
pub(super) const fn compute_stamp_radius(expected_fwhm: f32) -> usize {
    let radius = (expected_fwhm * STAMP_RADIUS_FWHM_FACTOR).ceil() as usize;
    if radius < MIN_STAMP_RADIUS {
        MIN_STAMP_RADIUS
    } else if radius > MAX_STAMP_RADIUS {
        MAX_STAMP_RADIUS
    } else {
        radius
    }
}

/// Check if position is within valid bounds for stamp extraction.
#[inline]
pub(super) fn is_valid_stamp_position(pos: DVec2, size: Size2us, stamp_radius: usize) -> bool {
    let icx = pos.x.round() as isize;
    let icy = pos.y.round() as isize;
    icx >= stamp_radius as isize
        && icy >= stamp_radius as isize
        && icx < (size.width - stamp_radius) as isize
        && icy < (size.height - stamp_radius) as isize
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

/// Measure a star candidate: compute sub-pixel position and quality metrics.
///
/// This is the main entry point for the measurement stage. It takes a detected
/// region and computes:
/// - Sub-pixel position using the configured centroid method
/// - Quality metrics: flux, FWHM, eccentricity, SNR, sharpness, roundness
///
/// Returns `None` if the candidate fails quality checks during measurement.
///
/// # Centroid Methods
///
/// The position refinement method is selected via `config.centroid_method`:
/// - `WeightedMoments`: Iterative weighted centroid (~0.05 pixel accuracy, fast)
/// - `GaussianFit`: 2D Gaussian fitting (~0.01 pixel accuracy, slower)
/// - `MoffatFit`: 2D Moffat fitting (~0.01 pixel accuracy, best for atmospheric seeing)
pub(super) fn measure_star(
    residual: &Buffer2<f32>,
    sky: &SkyNoise,
    saturation: &BitBuffer2,
    region: &Region,
    config: &MeasurementConfig,
    expected_fwhm: f32,
    grid: &StampGrid,
) -> Option<Star> {
    // Built once per detection by the measure stage, from this same `expected_fwhm`.
    let stamp_radius = grid.radius;
    debug_assert_eq!(stamp_radius, compute_stamp_radius(expected_fwhm));

    let mut pos = DVec2::new(region.peak.x as f64, region.peak.y as f64);

    // A fit that follows needs only 2 moments steps: it converges to the same optimum from any
    // seed the moments leave (`moments_contract_and_fits_ignore_the_seed`).
    let phase1_iters = match config.centroid_method {
        CentroidMethod::WeightedMoments => MAX_MOMENTS_ITERATIONS,
        CentroidMethod::GaussianFit | CentroidMethod::MoffatFit { .. } => {
            MOMENTS_ITERATIONS_BEFORE_FIT
        }
    };
    pos = moments_centroid(residual, pos, stamp_radius, expected_fwhm, phase1_iters)?;

    // The global map's sky is already out of the residual, so with no annulus the stamp is
    // measured as it stands, against the map's noise at the star.
    let global_fallback = || LocalBackground {
        offset: 0.0,
        noise: sky.noise.row(pos.y.round() as usize)[pos.x.round() as usize],
    };

    // None in GlobalMap mode, or when fewer than 10 annulus pixels lie in the frame — which a
    // centre at a stamp that fits never meets, since it keeps at least 8r of them. A missing
    // annulus falls back to the map's for the fit seed below, but must NOT become a metrics
    // override: the map's noise at one pixel stands in for the stamp only where nothing better was
    // measured.
    let annulus_at = |at: DVec2| match config.local_background {
        LocalBackgroundMethod::GlobalMap => None,
        LocalBackgroundMethod::LocalAnnulus => compute_annulus_background(
            residual,
            at,
            stamp_radius,
            annulus_outer_radius(stamp_radius),
        ),
    };
    let moments_pos = pos;
    let annulus_background = annulus_at(pos);
    let LocalBackground {
        offset: local_offset,
        noise: local_noise,
    } = annulus_background.unwrap_or_else(global_fallback);

    // A converged fit's widths replace the moment-based FWHM and eccentricity.
    let mut fit_fwhm: Option<f32> = None;
    let mut fit_eccentricity: Option<f32> = None;

    let fit_noise = config.noise_model.map(|noise_model| FitNoise {
        sky_noise: local_noise,
        noise_model,
    });

    match config.centroid_method {
        // The fits run to full convergence, not just the centre's: their widths are read as the
        // star's FWHM and eccentricity.
        CentroidMethod::GaussianFit => {
            let fit = GaussianFit::new(
                residual,
                pos,
                grid,
                local_offset,
                fit_noise,
                &GaussianFitConfig::default(),
            );
            if let Some(result) = fit.filter(|r| r.converged) {
                pos = result.pos;
                fit_fwhm = Some(result.covariance.fwhm());
                fit_eccentricity = Some(result.covariance.eccentricity());
            }
        }
        CentroidMethod::MoffatFit { beta } => {
            let fit_config = MoffatFitConfig {
                fixed_beta: beta,
                lm: lm_optimizer::LMConfig::default(),
            };
            let fit = MoffatFit::new(residual, pos, grid, local_offset, fit_noise, &fit_config);
            if let Some(result) = fit.filter(|r| r.converged) {
                pos = result.pos;
                fit_fwhm = Some(result.fwhm);
                // Moffat is radially symmetric (single alpha) — eccentricity stays moment-based
            }
        }
        CentroidMethod::WeightedMoments => {}
    }

    // The estimate above was centred on the moments position, and the fit has since moved the
    // star. `compute_annulus_background` samples by rounded centre, so re-running it only changes
    // anything once the fit crosses a pixel boundary — the normal sub-pixel move would resample
    // exactly the same ring. Rare, so this costs almost nothing; skipping it would measure flux
    // and SNR against a sky annulus centred on the wrong pixel.
    let annulus_background = if pos.round() == moments_pos.round() {
        annulus_background
    } else {
        annulus_at(pos)
    };

    // Flux, SNR, sharpness and roundness come from the stamp whatever the method.
    let mut star = compute_star(
        residual,
        sky,
        pos,
        region.peak_value,
        stamp_radius,
        annulus_background,
        config.noise_model.as_ref(),
    )?;
    star.saturated = saturation.get_at(region.peak);

    if let Some(fwhm) = fit_fwhm {
        star.fwhm = fwhm;
    }
    if let Some(ecc) = fit_eccentricity {
        star.eccentricity = ecc;
    }

    Some(star)
}

/// The outer radius of the sky annulus around a stamp of `stamp_radius`: half as far again, so
/// the annulus is as wide as the stamp is deep and holds enough sky pixels to clip.
const fn annulus_outer_radius(stamp_radius: usize) -> usize {
    (3 * stamp_radius).div_ceil(2)
}

/// Weighted-moments centroid from `start`: at most `iterations` steps of [`refine_centroid`],
/// stopping once a step moves less than `CENTROID_CONVERGENCE_THRESHOLD`. `None` when a step
/// leaves the frame or finds no flux.
fn moments_centroid(
    residual: &Buffer2<f32>,
    start: DVec2,
    stamp_radius: usize,
    expected_fwhm: f32,
    iterations: usize,
) -> Option<DVec2> {
    let mut pos = start;
    for _ in 0..iterations {
        let new_pos = refine_centroid(residual, pos, stamp_radius, expected_fwhm)?;
        let delta = new_pos - pos;
        pos = new_pos;
        if delta.length_squared() < CONVERGENCE_THRESHOLD_SQ {
            break;
        }
    }
    Some(pos)
}

/// Single iteration of centroid refinement using Gaussian-weighted moments.
///
/// Returns the new position or None if position is invalid.
/// Uses f64 accumulators for numerical stability.
fn refine_centroid(
    residual: &Buffer2<f32>,
    pos: DVec2,
    stamp_radius: usize,
    expected_fwhm: f32,
) -> Option<DVec2> {
    let size = Size2us::new(residual.width(), residual.height());
    if !is_valid_stamp_position(pos, size, stamp_radius) {
        return None;
    }

    let icx = pos.x.round() as isize;
    let icy = pos.y.round() as isize;

    // 0.8 of the expected σ: a tighter window weights the noisy wings less.
    let sigma = (fwhm_to_sigma(expected_fwhm) * 0.8).clamp(1.0, stamp_radius as f32 * 0.5);
    let two_sigma_sq = 2.0 * f64::from(sigma) * f64::from(sigma);

    let mut sum_x = 0.0f64;
    let mut sum_y = 0.0f64;
    let mut sum_w = 0.0f64;

    let pos_x = pos.x;
    let pos_y = pos.y;

    let stamp_radius_i32 = stamp_radius as i32;
    let stamp_size = 2 * stamp_radius + 1;
    let x0 = icx as usize - stamp_radius;

    // The weight is a circular Gaussian, so it factors per axis:
    // `exp(-(dx² + dy²)/2σ²) = exp(-dx²/2σ²) · exp(-dy²/2σ²)`. Filling one column vector here and
    // one row scalar below turns `(2r+1)²` `exp` calls into `2(2r+1)` — 961 into 62 at the largest
    // stamp — and this loop is what `measure_star` spends most of its time in. Costs one extra
    // multiply per pixel and a ulp or two of weight precision against the unfactored form.
    //
    // `column_moments[c]` is `c · column_weights[c]`, prebuilt so the inner loop is two dot
    // products over the row and never converts a pixel index to f64.
    let mut column_weights = [0.0f64; MAX_STAMP_SIZE];
    let mut column_moments = [0.0f64; MAX_STAMP_SIZE];
    for column in 0..stamp_size {
        let ddx = (x0 + column) as f64 - pos_x;
        let weight = (-ddx * ddx / two_sigma_sq).exp();
        column_weights[column] = weight;
        column_moments[column] = column as f64 * weight;
    }

    for dy in -stamp_radius_i32..=stamp_radius_i32 {
        let y = (icy + dy as isize) as usize;
        // One bounds check per row rather than per pixel — `is_valid_stamp_position` above has
        // already established the whole stamp is inside the frame.
        let px_row = &residual.row(y)[x0..x0 + stamp_size];

        let py = y as f64;
        let ddy = py - pos_y;
        let row_weight = (-ddy * ddy / two_sigma_sq).exp();

        // The row's total weight, and its first moment about the stamp's left edge. `row_weight`
        // and `py` are constant across the row, so they scale these two totals once at the bottom
        // instead of multiplying into every pixel.
        let mut row_w = 0.0f64;
        let mut row_x = 0.0f64;
        for ((&value, &column_weight), &column_moment) in px_row
            .iter()
            .zip(&column_weights[..stamp_size])
            .zip(&column_moments[..stamp_size])
        {
            let signal = f64::from(value.max(0.0));
            row_w += signal * column_weight;
            row_x += signal * column_moment;
        }

        let weighted_row = row_weight * row_w;
        sum_w += weighted_row;
        sum_y += weighted_row * py;
        sum_x += row_weight * row_x;
    }

    if sum_w < f64::EPSILON {
        return None;
    }

    // `sum_x` is the first moment about the stamp's left edge, so lift it back into image x.
    let new_pos = DVec2::new(x0 as f64 + sum_x / sum_w, sum_y / sum_w);

    // A step this far means the stamp holds something other than one star.
    let max_move = stamp_size as f64 / 4.0;
    if (new_pos - pos).abs().max_element() > max_move {
        return None;
    }

    Some(new_pos)
}

/// Symmetric 2×2 covariance (px²) for windowed second moments.
/// Construct a star and compute its quality metrics at the given position.
///
/// Uses f64 accumulators for numerical stability.
///
/// If `noise_model` is provided, uses the full CCD noise equation:
/// `SNR = flux / sqrt(flux/G + npix × (σ_sky² + (read_noise_electrons/G)²))`,
/// where `G` is electrons per normalized unit.
///
/// Otherwise, uses the simplified background-dominated formula:
/// `SNR = flux / (σ_sky × sqrt(npix))`
///
/// `local`, when set, is the [`LocalBackgroundMethod::LocalAnnulus`] estimate: a sky offset the
/// residual still carries at this stamp and the stamp's own noise, both valid at the stamp scale.
/// It applies to every consumer here — flux/marginals, the windowed covariance behind
/// FWHM/eccentricity, and the SNR noise — so all metrics share one sky convention.
fn compute_star(
    residual: &Buffer2<f32>,
    sky: &SkyNoise,
    pos: DVec2,
    peak: f32,
    stamp_radius: usize,
    local: Option<LocalBackground>,
    noise_model: Option<&NoiseModel>,
) -> Option<Star> {
    let width = residual.width();
    let height = residual.height();
    let offset = local.map_or(0.0, |local| local.offset);

    if !is_valid_stamp_position(pos, Size2us::new(width, height), stamp_radius) {
        return None;
    }

    let icx = pos.x.round() as isize;
    let icy = pos.y.round() as isize;

    // Flux, core flux and peak sum the *signed* residual: sky noise is zero-mean, and clipping each
    // pixel at zero would turn it into a positive bias of about 0.4σ per pixel. The second moments
    // and the marginals are weights, and a weight cannot be negative, so they take the clipped
    // residual and normalize by its own sum.
    let mut flux = 0.0f64;
    let mut weight_sum = 0.0f64;
    let mut core_flux = 0.0f64;
    let mut sum_x2 = 0.0f64;
    let mut sum_y2 = 0.0f64;
    let mut sum_xy = 0.0f64;
    let mut noise_sum = 0.0f64;
    let mut noise_count = 0usize;
    let mut peak_value = f64::NEG_INFINITY;

    let stamp_size = 2 * stamp_radius + 1;
    let mut marginal_x = [0.0f64; MAX_STAMP_SIZE];
    let mut marginal_y = [0.0f64; MAX_STAMP_SIZE];

    let stamp_radius_i32 = stamp_radius as i32;
    let outer_ring_threshold = (stamp_radius_i32 - 2) * (stamp_radius_i32 - 2);
    for dy in -stamp_radius_i32..=stamp_radius_i32 {
        let y = (icy + dy as isize) as usize;
        let px_row = residual.row(y);
        let noise_row = sky.noise.row(y);
        for dx in -stamp_radius_i32..=stamp_radius_i32 {
            let x = (icx + dx as isize) as usize;

            let signal = f64::from(px_row[x] - offset);
            let value = signal.max(0.0);

            flux += signal;
            weight_sum += value;
            peak_value = peak_value.max(signal);

            if dx.abs() <= 1 && dy.abs() <= 1 {
                core_flux += signal;
            }

            let mx_idx = (dx + stamp_radius_i32) as usize;
            let my_idx = (dy + stamp_radius_i32) as usize;
            marginal_x[mx_idx] += value;
            marginal_y[my_idx] += value;

            // Weighted second moments for FWHM and eccentricity. Kept in this loop rather than
            // recomputed on the rare `windowed_covariance` failure below: a second traversal there
            // measured no better than these three multiply-adds, which are a small share of an
            // already branchy loop body.
            let fx = x as f64 - pos.x;
            let fy = y as f64 - pos.y;
            sum_x2 += value * fx * fx;
            sum_y2 += value * fy * fy;
            sum_xy += value * fx * fy;

            let r2 = dx * dx + dy * dy;
            if local.is_none() && r2 > outer_ring_threshold {
                noise_sum += f64::from(noise_row[x]);
                noise_count += 1;
            }
        }
    }

    // No net signal above the sky, or nothing positive to weight the moments with: not a star.
    if flux < f64::EPSILON || weight_sum < f64::EPSILON {
        return None;
    }

    // Adaptive windowed second moments: Gaussian-weight by an iteratively-matched
    // window to suppress wing noise, then deconvolve the window so FWHM/eccentricity
    // stay unbiased. Seed the window from the plain moment; fall back to the plain
    // moments if it can't converge to a valid (positive-definite) covariance.
    // `sum_x2 + sum_y2` is Σ value·r², the radial moment the window is seeded from.
    let seed_sigma_sq = ((sum_x2 + sum_y2) / weight_sum / 2.0).max(MIN_SIGMA_SQ);
    let cov =
        windowed_covariance(residual, offset, pos, stamp_radius, seed_sigma_sq).unwrap_or(Cov2 {
            xx: sum_x2 / weight_sum,
            yy: sum_y2 / weight_sum,
            xy: sum_xy / weight_sum,
        });

    let fwhm = cov.fwhm();
    let eccentricity = cov.eccentricity();

    let avg_noise = match local {
        Some(local) => local.noise,
        None if noise_count > 0 => (noise_sum / noise_count as f64) as f32,
        None => sky.noise.row(icy as usize)[icx as usize],
    };

    let npix = (2 * stamp_radius + 1).pow(2);
    let flux_f32 = flux as f32;

    let snr = compute_snr(flux_f32, avg_noise, npix, noise_model);

    let sharpness = if core_flux > f64::EPSILON {
        (peak_value / core_flux).clamp(0.0, 1.0) as f32
    } else {
        1.0
    };

    Some(Star {
        pos,
        flux: flux_f32,
        fwhm,
        eccentricity,
        snr,
        peak,
        saturated: false,
        sharpness,
        roundness: Roundness::from_marginals(&marginal_x[..stamp_size], &marginal_y[..stamp_size]),
    })
}

/// Compute SNR using the configured sensor noise model when available.
///
/// Uses the full CCD noise equation when the model is provided:
/// `SNR = flux / sqrt(flux/G + npix × (σ_sky² + (read_noise_electrons/G)²))`,
/// where `G` is electrons per normalized unit.
///
/// Otherwise, uses simplified background-dominated formula:
/// `SNR = flux / (σ_sky × sqrt(npix))`
fn compute_snr(flux: f32, sky_noise: f32, npix: usize, noise_model: Option<&NoiseModel>) -> f32 {
    let sky_var = sky_noise * sky_noise;

    let total_var = match noise_model {
        Some(noise) => {
            noise.variance_normalized(f64::from(flux), f64::from(sky_noise), npix) as f32
        }
        None => npix as f32 * sky_var,
    };

    // Floor the variance rather than branching on it: dividing by `total_var` in one arm and by
    // its square root in the other put a 2896× step at the boundary. `max` also swallows a
    // non-finite variance, since it returns the other operand for NaN.
    flux / total_var.max(f32::EPSILON).sqrt()
}

#[cfg(all(test, feature = "bench"))]
mod bench;
#[cfg(test)]
mod tests;
