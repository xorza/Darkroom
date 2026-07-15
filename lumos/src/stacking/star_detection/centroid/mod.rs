//! Sub-pixel centroid computation and star quality metrics.
//!
//! Uses iterative weighted centroid algorithm for sub-pixel accurate positioning,
//! typically achieving ~0.05 pixel accuracy.
//!
//! Also provides 2D Gaussian and Moffat profile fitting for higher precision
//! centroid computation (~0.01 pixel accuracy).
//!
//! All fitting and accumulation operations use f64 for numerical stability.

pub mod gaussian_fit;
mod linear_solver;
mod lm_optimizer;
pub mod moffat_fit;

#[cfg(test)]
mod bench;
#[cfg(test)]
pub(crate) mod test_utils;
#[cfg(test)]
mod tests;

use arrayvec::ArrayVec;
use glam::Vec2;

use crate::math::statistics::{ClippedStats, sigma_clipped_median_mad_arrayvec};
use crate::math::{FWHM_TO_SIGMA, sigma_to_fwhm};
use crate::stacking::star_detection::background::estimate::BackgroundEstimate;
use crate::stacking::star_detection::config::{
    CentroidMethod, LocalBackgroundMethod, MeasurementConfig,
};
use crate::stacking::star_detection::deblend::region::Region;
use crate::stacking::star_detection::star::Star;
use gaussian_fit::{GaussianFitConfig, fit_gaussian_2d};
use imaginarium::Buffer2;
use moffat_fit::{MoffatFitConfig, fit_moffat_2d};

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

/// Maximum stamp pixels (31×31 for stamp_radius=15).
const MAX_STAMP_PIXELS: usize = (2 * MAX_STAMP_RADIUS + 1).pow(2);

/// Maximum annulus outer radius (1.5 × MAX_STAMP_RADIUS, rounded up).
const MAX_ANNULUS_OUTER_RADIUS: usize = (MAX_STAMP_RADIUS * 3).div_ceil(2); // = 23

/// Maximum annulus pixels for LocalAnnulus background method.
/// Computed as the area of a square with side 2×outer_radius+1.
const MAX_ANNULUS_PIXELS: usize = (2 * MAX_ANNULUS_OUTER_RADIUS + 1).pow(2); // = 47² = 2209

/// Centroid convergence threshold in pixels.
///
/// Iteration stops when the distance moved is less than this value.
/// Set to 0.0001 (0.1 millipixel) for sub-pixel astrometric precision.
const CENTROID_CONVERGENCE_THRESHOLD: f32 = 0.0001;

/// Maximum weighted-moments iterations for standalone centroid (no fitting follows).
pub(crate) const MAX_MOMENTS_ITERATIONS: usize = 10;

/// Weighted-moments iterations when L-M fitting follows.
/// Only needs to provide a rough seed — L-M refines position independently.
const MOMENTS_ITERATIONS_BEFORE_FIT: usize = 2;

/// Convergence threshold in pixels squared.
pub(crate) const CONVERGENCE_THRESHOLD_SQ: f32 =
    CENTROID_CONVERGENCE_THRESHOLD * CENTROID_CONVERGENCE_THRESHOLD;

/// Compute stamp radius from expected FWHM.
#[inline]
fn compute_stamp_radius(expected_fwhm: f32) -> usize {
    let radius = (expected_fwhm * STAMP_RADIUS_FWHM_FACTOR).ceil() as usize;
    radius.clamp(MIN_STAMP_RADIUS, MAX_STAMP_RADIUS)
}

/// Check if position is within valid bounds for stamp extraction.
#[inline]
pub(crate) fn is_valid_stamp_position(
    pos: Vec2,
    width: usize,
    height: usize,
    stamp_radius: usize,
) -> bool {
    let icx = pos.x.round() as isize;
    let icy = pos.y.round() as isize;
    icx >= stamp_radius as isize
        && icy >= stamp_radius as isize
        && icx < (width - stamp_radius) as isize
        && icy < (height - stamp_radius) as isize
}

/// Stack-allocated stamp data extracted around a star candidate.
/// Uses ArrayVec to avoid heap allocations for typical stamp sizes.
#[derive(Debug)]
pub(crate) struct StampData {
    /// X coordinates of stamp pixels (relative to image origin).
    pub x: ArrayVec<f32, MAX_STAMP_PIXELS>,
    /// Y coordinates of stamp pixels.
    pub y: ArrayVec<f32, MAX_STAMP_PIXELS>,
    /// Pixel values (background-subtracted at the caller if needed).
    pub z: ArrayVec<f32, MAX_STAMP_PIXELS>,
    /// Peak pixel value within the stamp.
    pub peak: f32,
}

/// Extract a square stamp of pixel data around a position.
///
/// Returns [`StampData`] or None if position is outside the valid stamp region.
/// Uses stack-allocated ArrayVec to avoid heap allocations.
pub(crate) fn extract_stamp(
    pixels: &Buffer2<f32>,
    pos: Vec2,
    stamp_radius: usize,
) -> Option<StampData> {
    let width = pixels.width();
    let height = pixels.height();

    if !is_valid_stamp_position(pos, width, height, stamp_radius) {
        return None;
    }

    let icx = pos.x.round() as isize;
    let icy = pos.y.round() as isize;
    let stamp_radius_i32 = stamp_radius as i32;
    let mut data_x = ArrayVec::new();
    let mut data_y = ArrayVec::new();
    let mut data_z = ArrayVec::new();
    let mut peak_value = f32::MIN;

    for dy in -stamp_radius_i32..=stamp_radius_i32 {
        for dx in -stamp_radius_i32..=stamp_radius_i32 {
            let x = (icx + dx as isize) as usize;
            let y = (icy + dy as isize) as usize;
            let value = pixels.row(y)[x];

            data_x.push(x as f32);
            data_y.push(y as f32);
            data_z.push(value);
            peak_value = peak_value.max(value);
        }
    }

    Some(StampData {
        x: data_x,
        y: data_y,
        z: data_z,
        peak: peak_value,
    })
}

/// Estimate sigma from weighted second moments of the stamp data.
///
/// For a Gaussian: E[r²] = 2σ², so σ = sqrt(E[r²]/2)
/// This gives a better initial guess for L-M optimization than a fixed value.
pub(crate) fn estimate_sigma_from_moments(
    data_x: &[f32],
    data_y: &[f32],
    data_z: &[f32],
    pos: Vec2,
    background: f32,
) -> f32 {
    let mut sum_r2 = 0.0f64;
    let mut sum_w = 0.0f64;

    for ((&x, &y), &z) in data_x.iter().zip(data_y.iter()).zip(data_z.iter()) {
        let w = (z - background).max(0.0) as f64;
        let dx = x as f64 - pos.x as f64;
        let dy = y as f64 - pos.y as f64;
        let r2 = dx * dx + dy * dy;
        sum_r2 += w * r2;
        sum_w += w;
    }

    if sum_w > f64::EPSILON {
        // For Gaussian: E[r²] = 2σ², so σ = sqrt(E[r²]/2)
        ((sum_r2 / sum_w / 2.0).sqrt()).clamp(0.5, 10.0) as f32
    } else {
        2.0 // fallback
    }
}

/// Per-pixel inverse-variance weights for the LM fit, using the CCD noise model
/// (same per-pixel decomposition as `compute_snr`):
/// `w_i = 1 / (max(z_i − bg, 0)/gain + sky_noise² + read_noise²/gain²)`.
///
/// Down-weights the shot-noisy bright core so the fit is the ML estimator instead of
/// over-weighting high-signal pixels (which biases the sub-pixel centroid/FWHM/flux).
pub(crate) fn inverse_variance_weights(
    data_z: &[f64],
    background: f64,
    sky_noise: f64,
    gain: f64,
    read_noise: f64,
) -> ArrayVec<f64, MAX_STAMP_PIXELS> {
    let sky_var = sky_noise * sky_noise;
    let read_var = read_noise * read_noise / (gain * gain);
    data_z
        .iter()
        .map(|&z| {
            let signal = (z - background).max(0.0);
            1.0 / (signal / gain + sky_var + read_var).max(1e-12)
        })
        .collect()
}

/// Noise inputs for an inverse-variance-weighted fit: the local sky σ plus the
/// `NoiseModel`'s gain/read-noise. `None` (absent) means an unweighted fit.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FitNoise {
    pub sky_noise: f32,
    pub gain: f32,
    pub read_noise: f32,
}

/// Per-pixel fit weights for a stamp, or `None` for an unweighted fit;
/// see [`inverse_variance_weights`].
pub(crate) fn fit_weights(
    data_z: &[f64],
    background: f32,
    noise: Option<FitNoise>,
) -> Option<ArrayVec<f64, MAX_STAMP_PIXELS>> {
    noise.map(|n| {
        inverse_variance_weights(
            data_z,
            background as f64,
            n.sky_noise as f64,
            n.gain as f64,
            n.read_noise as f64,
        )
    })
}

/// Flat per-stamp sky estimate: one (background, noise) pair valid at the stamp
/// scale, as opposed to the per-pixel tiled global map.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LocalBackground {
    pub bg: f32,
    pub noise: f32,
}

/// Compute local background and noise using an annular region around the star.
///
/// The inner radius excludes the star's flux, and the outer radius samples
/// the local sky. Uses sigma-clipped median for robustness.
///
/// # Arguments
/// * `pixels` - Image data
/// * `width` - Image width
/// * `height` - Image height
/// * `pos` - Star center position
/// * `inner_radius` - Inner radius of annulus (excludes star)
/// * `outer_radius` - Outer radius of annulus
///
/// # Returns
/// The local background/noise, or None if not enough valid pixels
fn compute_annulus_background(
    pixels: &[f32],
    width: usize,
    height: usize,
    pos: Vec2,
    inner_radius: usize,
    outer_radius: usize,
) -> Option<LocalBackground> {
    let icx = pos.x.round() as isize;
    let icy = pos.y.round() as isize;
    let inner_r2 = (inner_radius * inner_radius) as f32;
    let outer_r2 = (outer_radius * outer_radius) as f32;

    // Use stack-allocated ArrayVec to avoid heap allocation
    let mut values: ArrayVec<f32, MAX_ANNULUS_PIXELS> = ArrayVec::new();

    let outer_r_i32 = outer_radius as i32;
    for dy in -outer_r_i32..=outer_r_i32 {
        for dx in -outer_r_i32..=outer_r_i32 {
            let r2 = (dx * dx + dy * dy) as f32;
            if r2 < inner_r2 || r2 > outer_r2 {
                continue;
            }

            let x = icx + dx as isize;
            let y = icy + dy as isize;

            if x >= 0 && x < width as isize && y >= 0 && y < height as isize {
                values.push(pixels[y as usize * width + x as usize]);
            }
        }
    }

    if values.len() < 10 {
        return None;
    }

    // Sigma-clipped median (2 iterations, 3-sigma clip)
    let stats = sigma_clipped_median_mad(&mut values, 3.0, 2);
    Some(LocalBackground {
        bg: stats.median,
        noise: stats.sigma,
    })
}

/// Compute sigma-clipped median and MAD using the shared implementation.
/// Uses stack-allocated ArrayVec for deviations to avoid heap allocation.
#[inline]
fn sigma_clipped_median_mad(values: &mut [f32], kappa: f32, iterations: usize) -> ClippedStats {
    let mut deviations: ArrayVec<f32, MAX_ANNULUS_PIXELS> = ArrayVec::new();
    // Resize to match values length
    deviations.extend(std::iter::repeat_n(0.0, values.len()));
    sigma_clipped_median_mad_arrayvec(values, &mut deviations, kappa, iterations)
}

/// Measure a star candidate: compute sub-pixel position and quality metrics.
///
/// This is the main entry point for the measurement stage. It takes a detected
/// region and computes:
/// - Sub-pixel position using the configured centroid method
/// - Quality metrics: flux, FWHM, eccentricity, SNR, sharpness, roundness
/// - Laplacian SNR for cosmic ray detection
///
/// Returns `None` if the candidate fails quality checks during measurement.
///
/// # Centroid Methods
///
/// The position refinement method is selected via `config.centroid_method`:
/// - `WeightedMoments`: Iterative weighted centroid (~0.05 pixel accuracy, fast)
/// - `GaussianFit`: 2D Gaussian fitting (~0.01 pixel accuracy, slower)
/// - `MoffatFit`: 2D Moffat fitting (~0.01 pixel accuracy, best for atmospheric seeing)
pub fn measure_star(
    pixels: &Buffer2<f32>,
    background: &BackgroundEstimate,
    region: &Region,
    config: &MeasurementConfig,
    expected_fwhm: f32,
) -> Option<Star> {
    let width = pixels.width();
    let height = pixels.height();
    // Compute adaptive stamp radius based on expected FWHM
    let stamp_radius = compute_stamp_radius(expected_fwhm);

    // Initial position from peak
    let mut pos = Vec2::new(region.peak.x as f32, region.peak.y as f32);

    // First pass: weighted moments for initial refinement.
    // When a fitting method follows, only 2 iterations are needed — the L-M
    // optimizer refines position independently and converges to the same result
    // regardless of Phase 1 precision (verified by tests).
    let phase1_iters = match config.centroid_method {
        CentroidMethod::WeightedMoments => MAX_MOMENTS_ITERATIONS,
        CentroidMethod::GaussianFit | CentroidMethod::MoffatFit { .. } => {
            MOMENTS_ITERATIONS_BEFORE_FIT
        }
    };
    for _ in 0..phase1_iters {
        let new_pos = refine_centroid(
            pixels,
            width,
            height,
            background,
            pos,
            stamp_radius,
            expected_fwhm,
        )?;

        let delta = new_pos - pos;
        pos = new_pos;

        if delta.length_squared() < CONVERGENCE_THRESHOLD_SQ {
            break;
        }
    }

    // Compute local background based on configured method
    let icx = pos.x.round() as isize;
    let icy = pos.y.round() as isize;
    let bg_y = icy as usize;
    let bg_x = icx as usize;
    let global_fallback = || LocalBackground {
        bg: background.background.row(bg_y)[bg_x],
        noise: background.noise.row(bg_y)[bg_x],
    };

    // The annulus estimate; None in GlobalMap mode or when the annulus has too few
    // in-bounds samples (star near an edge). A failed annulus falls back to the
    // center pixel of the global map for the fit seed below, but must NOT become a
    // metrics override: flattening one map pixel across the whole stamp would be
    // strictly worse than the per-pixel map itself.
    let annulus_background = match config.local_background {
        LocalBackgroundMethod::GlobalMap => None,
        LocalBackgroundMethod::LocalAnnulus => {
            let inner_radius = stamp_radius;
            let outer_radius = (stamp_radius as f32 * 1.5).ceil() as usize;
            compute_annulus_background(pixels, width, height, pos, inner_radius, outer_radius)
        }
    };
    let LocalBackground {
        bg: local_bg,
        noise: local_noise,
    } = annulus_background.unwrap_or_else(global_fallback);

    // Refine with profile fitting if requested.
    // When fit converges, also extract FWHM and eccentricity from fit parameters
    // (more accurate than moment-based estimates).
    let mut fit_fwhm: Option<f32> = None;
    let mut fit_eccentricity: Option<f32> = None;

    // Inverse-variance fit weights when a noise model is configured (PR1).
    let fit_noise = config.noise_model.as_ref().map(|nm| FitNoise {
        sky_noise: local_noise,
        gain: nm.gain,
        read_noise: nm.read_noise,
    });

    match config.centroid_method {
        CentroidMethod::GaussianFit => {
            let fit_config = GaussianFitConfig {
                position_convergence_threshold: CENTROID_CONVERGENCE_THRESHOLD as f64,
                ..GaussianFitConfig::default()
            };
            let fit = fit_gaussian_2d(pixels, pos, stamp_radius, local_bg, fit_noise, &fit_config);
            if let Some(result) = fit.filter(|r| r.converged) {
                pos = result.pos;
                // FWHM from geometric mean of sigma_x, sigma_y
                let geo_sigma = (result.sigma.x * result.sigma.y).sqrt();
                fit_fwhm = Some(sigma_to_fwhm(geo_sigma));
                // Eccentricity from sigma ratio: e = sqrt(1 - (min/max)^2)
                let (s_min, s_max) = if result.sigma.x < result.sigma.y {
                    (result.sigma.x, result.sigma.y)
                } else {
                    (result.sigma.y, result.sigma.x)
                };
                if s_max > f32::EPSILON {
                    let ratio = s_min / s_max;
                    fit_eccentricity = Some((1.0 - ratio * ratio).sqrt().clamp(0.0, 1.0));
                }
            }
        }
        CentroidMethod::MoffatFit { beta } => {
            let fit_config = MoffatFitConfig {
                fixed_beta: beta,
                lm: lm_optimizer::LMConfig {
                    position_convergence_threshold: CENTROID_CONVERGENCE_THRESHOLD as f64,
                    ..lm_optimizer::LMConfig::default()
                },
            };
            let fit = fit_moffat_2d(pixels, pos, stamp_radius, local_bg, fit_noise, &fit_config);
            if let Some(result) = fit.filter(|r| r.converged) {
                pos = result.pos;
                fit_fwhm = Some(result.fwhm);
                // Moffat is radially symmetric (single alpha) — eccentricity stays moment-based
            }
        }
        CentroidMethod::WeightedMoments => {
            // Already computed above
        }
    };

    // Compute quality metrics (flux, SNR, sharpness, roundness always from moments)
    let (gain, read_noise) = config
        .noise_model
        .as_ref()
        .map(|nm| (Some(nm.gain), Some(nm.read_noise)))
        .unwrap_or((None, None));

    let mut metrics = compute_metrics(
        pixels,
        background,
        pos,
        stamp_radius,
        annulus_background,
        gain,
        read_noise,
    )?;

    // Override FWHM and eccentricity with fit-derived values when available
    if let Some(fwhm) = fit_fwhm {
        metrics.fwhm = fwhm;
    }
    if let Some(ecc) = fit_eccentricity {
        metrics.eccentricity = ecc;
    }

    Some(Star {
        pos: pos.as_dvec2(),
        flux: metrics.flux,
        fwhm: metrics.fwhm,
        eccentricity: metrics.eccentricity,
        snr: metrics.snr,
        peak: region.peak_value,
        sharpness: metrics.sharpness,
        roundness1: metrics.roundness1,
        roundness2: metrics.roundness2,
    })
}

/// Single iteration of centroid refinement using Gaussian-weighted moments.
///
/// Returns the new position or None if position is invalid.
/// Uses f64 accumulators for numerical stability.
pub(crate) fn refine_centroid(
    pixels: &[f32],
    width: usize,
    height: usize,
    background: &BackgroundEstimate,
    pos: Vec2,
    stamp_radius: usize,
    expected_fwhm: f32,
) -> Option<Vec2> {
    if !is_valid_stamp_position(pos, width, height, stamp_radius) {
        return None;
    }

    let icx = pos.x.round() as isize;
    let icy = pos.y.round() as isize;

    // Adaptive sigma based on expected FWHM
    // sigma ≈ FWHM / FWHM_TO_SIGMA, use 0.8× for tighter weighting to reduce noise
    let sigma = (expected_fwhm / FWHM_TO_SIGMA * 0.8).clamp(1.0, stamp_radius as f32 * 0.5);
    let two_sigma_sq = 2.0 * (sigma as f64) * (sigma as f64);

    let mut sum_x = 0.0f64;
    let mut sum_y = 0.0f64;
    let mut sum_w = 0.0f64;

    let pos_x = pos.x as f64;
    let pos_y = pos.y as f64;

    let stamp_radius_i32 = stamp_radius as i32;
    for dy in -stamp_radius_i32..=stamp_radius_i32 {
        for dx in -stamp_radius_i32..=stamp_radius_i32 {
            let x = (icx + dx as isize) as usize;
            let y = (icy + dy as isize) as usize;
            let idx = y * width + x;

            // Background-subtracted value
            let value = (pixels[idx] - background.background[idx]).max(0.0) as f64;

            // Gaussian weight based on distance from current centroid
            let px = x as f64;
            let py = y as f64;
            let dist_sq = (px - pos_x) * (px - pos_x) + (py - pos_y) * (py - pos_y);
            let weight = value * (-dist_sq / two_sigma_sq).exp();

            sum_x += px * weight;
            sum_y += py * weight;
            sum_w += weight;
        }
    }

    if sum_w < f64::EPSILON {
        return None;
    }

    let new_x = (sum_x / sum_w) as f32;
    let new_y = (sum_y / sum_w) as f32;
    let new_pos = Vec2::new(new_x, new_y);

    // Reject if centroid moved too far (likely bad detection)
    let stamp_size = 2 * stamp_radius + 1;
    let max_move = stamp_size as f32 / 4.0;
    if (new_pos - pos).abs().max_element() > max_move {
        return None;
    }

    Some(new_pos)
}

/// Symmetric 2×2 covariance (px²) for windowed second moments.
#[derive(Debug, Clone, Copy)]
struct Cov2 {
    xx: f64,
    yy: f64,
    xy: f64,
}

impl Cov2 {
    fn trace(self) -> f64 {
        self.xx + self.yy
    }

    fn det(self) -> f64 {
        self.xx * self.yy - self.xy * self.xy
    }

    /// Inverse of the symmetric matrix, or `None` if (near-)singular.
    fn inverse(self) -> Option<Cov2> {
        let det = self.det();
        if det.abs() < 1e-12 {
            return None;
        }
        let inv = 1.0 / det;
        Some(Cov2 {
            xx: self.yy * inv,
            yy: self.xx * inv,
            xy: -self.xy * inv,
        })
    }
}

/// Window-scale bounds (px²): σ ∈ [0.5, 10] px.
const MIN_SIGMA_SQ: f64 = 0.25;
const MAX_SIGMA_SQ: f64 = 100.0;

/// Adaptive windowed second moments (SExtractor WIN style).
///
/// Weights the second moments by a circular Gaussian whose scale is iterated to
/// match the source (`σ_w² → trace(C)/2`), exponentially suppressing far-wing
/// noise, then deconvolves the window — `C = (C_obs⁻¹ − σ_w⁻²·I)⁻¹` — so the
/// result stays unbiased. Uses the unclamped signed `(px − bg)`: the window already
/// kills the wings, so noise cancels instead of rectifying and inflating
/// eccentricity (the failure mode of plain signed moments over a fixed stamp).
///
/// Returns the source covariance, or `None` if it never reaches a valid
/// positive-definite estimate (caller falls back to the plain moments).
///
/// `background_override` replaces the per-pixel map with a flat stamp-level sky,
/// exactly as in [`compute_metrics`] — both must subtract the same background or
/// FWHM/eccentricity and flux/SNR would come from different sky conventions.
fn windowed_covariance(
    pixels: &Buffer2<f32>,
    background: &BackgroundEstimate,
    background_override: Option<LocalBackground>,
    pos: Vec2,
    stamp_radius: usize,
    seed_sigma_sq: f64,
) -> Option<Cov2> {
    const MAX_ITERS: usize = 4;

    let icx = pos.x.round() as isize;
    let icy = pos.y.round() as isize;
    let pos_x = pos.x as f64;
    let pos_y = pos.y as f64;
    let sr = stamp_radius as i32;

    let mut sigma_w_sq = seed_sigma_sq.clamp(MIN_SIGMA_SQ, MAX_SIGMA_SQ);
    let mut best: Option<Cov2> = None;

    for _ in 0..MAX_ITERS {
        let inv_two_sw = 1.0 / (2.0 * sigma_w_sq);
        let mut w_sum = 0.0f64;
        let mut mxx = 0.0f64;
        let mut myy = 0.0f64;
        let mut mxy = 0.0f64;

        for dy in -sr..=sr {
            let y = (icy + dy as isize) as usize;
            let px_row = pixels.row(y);
            let bg_row = background.background.row(y);
            for dx in -sr..=sr {
                let x = (icx + dx as isize) as usize;
                let fx = x as f64 - pos_x;
                let fy = y as f64 - pos_y;
                let r2 = fx * fx + fy * fy;
                let bg = match background_override {
                    Some(local) => local.bg,
                    None => bg_row[x],
                };
                let wv = (-r2 * inv_two_sw).exp() * (px_row[x] - bg) as f64;
                w_sum += wv;
                mxx += wv * fx * fx;
                myy += wv * fy * fy;
                mxy += wv * fx * fy;
            }
        }

        if w_sum < f64::EPSILON {
            break;
        }
        let obs = Cov2 {
            xx: mxx / w_sum,
            yy: myy / w_sum,
            xy: mxy / w_sum,
        };

        // Deconvolve the circular window: C = (C_obs⁻¹ − σ_w⁻²·I)⁻¹
        let Some(obs_inv) = obs.inverse() else { break };
        let inv_sw = 1.0 / sigma_w_sq;
        let decon_inv = Cov2 {
            xx: obs_inv.xx - inv_sw,
            yy: obs_inv.yy - inv_sw,
            xy: obs_inv.xy,
        };
        let Some(c) = decon_inv.inverse() else { break };
        if c.det() <= 0.0 || c.trace() <= 0.0 {
            break;
        }

        let new_sigma_w_sq = (c.trace() / 2.0).clamp(MIN_SIGMA_SQ, MAX_SIGMA_SQ);
        let converged = (new_sigma_w_sq - sigma_w_sq).abs() < 1e-3 * sigma_w_sq;
        best = Some(c);
        sigma_w_sq = new_sigma_w_sq;
        if converged {
            break;
        }
    }

    best
}

/// Quality metrics for a star.
#[derive(Debug)]
pub(crate) struct StarMetrics {
    pub flux: f32,
    pub fwhm: f32,
    pub eccentricity: f32,
    pub snr: f32,
    pub sharpness: f32,
    /// GROUND: roundness from marginal Gaussian fits
    pub roundness1: f32,
    /// SROUND: roundness from symmetry analysis
    pub roundness2: f32,
}

/// Compute quality metrics for a star at the given position.
///
/// Uses f64 accumulators for numerical stability.
///
/// If `gain` and `read_noise` are provided, uses the full CCD noise equation:
/// `SNR = flux / sqrt(flux/gain + npix × (σ_sky² + σ_read²/gain²))`
///
/// Otherwise, uses the simplified background-dominated formula:
/// `SNR = flux / (σ_sky × sqrt(npix))`
///
/// `background_override`, when set, replaces the per-pixel tiled background/noise
/// map with a single flat estimate for the whole stamp — used for
/// [`LocalBackgroundMethod::LocalAnnulus`](crate::stacking::star_detection::config::LocalBackgroundMethod::LocalAnnulus),
/// whose locally-estimated sky level is only valid at the stamp scale, not
/// interpolated per pixel like the tiled map. It applies to every background
/// consumer here — flux/marginals, the windowed covariance behind FWHM/eccentricity,
/// and the SNR noise — so all metrics share one sky convention.
pub(crate) fn compute_metrics(
    pixels: &Buffer2<f32>,
    background: &BackgroundEstimate,
    pos: Vec2,
    stamp_radius: usize,
    background_override: Option<LocalBackground>,
    gain: Option<f32>,
    read_noise: Option<f32>,
) -> Option<StarMetrics> {
    let width = pixels.width();
    let height = pixels.height();

    if !is_valid_stamp_position(pos, width, height, stamp_radius) {
        return None;
    }

    let icx = pos.x.round() as isize;
    let icy = pos.y.round() as isize;

    // Collect background-subtracted values and positions (f64 accumulators)
    let mut flux = 0.0f64;
    let mut core_flux = 0.0f64;
    let mut sum_r2 = 0.0f64;
    let mut sum_x2 = 0.0f64;
    let mut sum_y2 = 0.0f64;
    let mut sum_xy = 0.0f64;
    let mut noise_sum = 0.0f64;
    let mut noise_count = 0usize;
    let mut peak_value = 0.0f64;

    // For roundness calculation: marginal sums
    let stamp_size = 2 * stamp_radius + 1;
    const MAX_STAMP_SIZE: usize = 2 * MAX_STAMP_RADIUS + 1; // 31
    let mut marginal_x = [0.0f64; MAX_STAMP_SIZE];
    let mut marginal_y = [0.0f64; MAX_STAMP_SIZE];

    let stamp_radius_i32 = stamp_radius as i32;
    let outer_ring_threshold = (stamp_radius_i32 - 2) * (stamp_radius_i32 - 2);
    for dy in -stamp_radius_i32..=stamp_radius_i32 {
        let y = (icy + dy as isize) as usize;
        let px_row = pixels.row(y);
        let bg_row = background.background.row(y);
        let noise_row = background.noise.row(y);
        for dx in -stamp_radius_i32..=stamp_radius_i32 {
            let x = (icx + dx as isize) as usize;

            let bg = match background_override {
                Some(local) => local.bg,
                None => bg_row[x],
            };
            let value = (px_row[x] - bg).max(0.0) as f64;

            flux += value;
            peak_value = peak_value.max(value);

            // Core flux for sharpness (3x3 region around center)
            if dx.abs() <= 1 && dy.abs() <= 1 {
                core_flux += value;
            }

            // Marginal distributions for roundness
            let mx_idx = (dx + stamp_radius_i32) as usize;
            let my_idx = (dy + stamp_radius_i32) as usize;
            marginal_x[mx_idx] += value;
            marginal_y[my_idx] += value;

            // Weighted second moments for FWHM and eccentricity
            let fx = x as f64 - pos.x as f64;
            let fy = y as f64 - pos.y as f64;
            sum_r2 += value * (fx * fx + fy * fy);
            sum_x2 += value * fx * fx;
            sum_y2 += value * fy * fy;
            sum_xy += value * fx * fy;

            // Collect noise from background region (outer ring)
            let r2 = dx * dx + dy * dy;
            if background_override.is_none() && r2 > outer_ring_threshold {
                noise_sum += noise_row[x] as f64;
                noise_count += 1;
            }
        }
    }

    if flux < f64::EPSILON {
        return None;
    }

    // Adaptive windowed second moments: Gaussian-weight by an iteratively-matched
    // window to suppress wing noise, then deconvolve the window so FWHM/eccentricity
    // stay unbiased. Seed the window from the plain moment; fall back to the plain
    // moments if it can't converge to a valid (positive-definite) covariance.
    let seed_sigma_sq = (sum_r2 / flux / 2.0).max(MIN_SIGMA_SQ);
    let cov = windowed_covariance(
        pixels,
        background,
        background_override,
        pos,
        stamp_radius,
        seed_sigma_sq,
    )
    .unwrap_or(Cov2 {
        xx: sum_x2 / flux,
        yy: sum_y2 / flux,
        xy: sum_xy / flux,
    });

    let trace = cov.trace();
    let det = cov.det();

    // FWHM from the mean second moment (assuming Gaussian PSF)
    let sigma_sq = (trace / 2.0).max(0.0);
    let fwhm = sigma_to_fwhm(sigma_sq.sqrt() as f32);

    // Eccentricity from covariance matrix eigenvalues
    let discriminant = (trace * trace - 4.0 * det).max(0.0);
    let lambda1 = (trace + discriminant.sqrt()) / 2.0;
    let lambda2 = (trace - discriminant.sqrt()) / 2.0;

    let eccentricity = if lambda1 > f64::EPSILON {
        (1.0 - lambda2 / lambda1).sqrt().clamp(0.0, 1.0) as f32
    } else {
        0.0
    };

    // Compute SNR using appropriate noise model
    let avg_noise = match background_override {
        Some(local) => local.noise,
        None if noise_count > 0 => (noise_sum / noise_count as f64) as f32,
        None => background.noise.row(icy as usize)[icx as usize],
    };

    let npix = (2 * stamp_radius + 1).pow(2) as f32;
    let flux_f32 = flux as f32;

    let snr = compute_snr(flux_f32, avg_noise, npix, gain, read_noise);

    // Sharpness = peak / core_flux
    let sharpness = if core_flux > f64::EPSILON {
        (peak_value / core_flux).clamp(0.0, 1.0) as f32
    } else {
        1.0
    };

    // Compute roundness metrics (DAOFIND style)
    let (roundness1, roundness2) =
        compute_roundness(&marginal_x[..stamp_size], &marginal_y[..stamp_size]);

    Some(StarMetrics {
        flux: flux_f32,
        fwhm,
        eccentricity,
        snr,
        sharpness,
        roundness1,
        roundness2,
    })
}

/// Compute DAOFIND-style roundness metrics.
///
/// Returns (GROUND, SROUND):
/// - GROUND: (Hx - Hy) / (Hx + Hy) where Hx, Hy are heights of marginal distributions
/// - SROUND: Symmetry-based roundness measuring bilateral asymmetry
fn compute_roundness(marginal_x: &[f64], marginal_y: &[f64]) -> (f32, f32) {
    // GROUND: Compare peak heights of marginal distributions
    let hx = marginal_x.iter().copied().fold(0.0f64, f64::max);
    let hy = marginal_y.iter().copied().fold(0.0f64, f64::max);
    let roundness1 = safe_ratio(hx - hy, hx + hy);

    // SROUND: Symmetry-based roundness using marginal distributions
    let center = marginal_x.len() / 2;

    // Compute asymmetry in x and y directions
    let (sum_left, sum_right) = split_sums(marginal_x, center);
    let (sum_top, sum_bottom) = split_sums(marginal_y, center);

    let asym_x = safe_ratio(sum_right - sum_left, sum_left + sum_right);
    let asym_y = safe_ratio(sum_bottom - sum_top, sum_top + sum_bottom);

    // SROUND is the RMS asymmetry
    let roundness2 = asym_x.hypot(asym_y);

    (
        (roundness1 as f32).clamp(-1.0, 1.0),
        (roundness2 as f32).clamp(0.0, 1.0),
    )
}

/// Compute sums of left and right halves of a slice (excluding center).
#[inline]
fn split_sums(slice: &[f64], center: usize) -> (f64, f64) {
    let left: f64 = slice[..center].iter().sum();
    let right: f64 = slice[center + 1..].iter().sum();
    (left, right)
}

/// Safe division returning 0.0 when denominator is near zero.
#[inline]
fn safe_ratio(numerator: f64, denominator: f64) -> f64 {
    if denominator > f64::EPSILON {
        numerator / denominator
    } else {
        0.0
    }
}

/// Compute SNR using appropriate noise model based on available parameters.
///
/// Uses full CCD noise equation when gain is provided:
/// `SNR = flux / sqrt(flux/gain + npix × (σ_sky² + σ_read²/gain²))`
///
/// Otherwise, uses simplified background-dominated formula:
/// `SNR = flux / (σ_sky × sqrt(npix))`
fn compute_snr(
    flux: f32,
    sky_noise: f32,
    npix: f32,
    gain: Option<f32>,
    read_noise: Option<f32>,
) -> f32 {
    let sky_var = sky_noise * sky_noise;

    let total_var = match (gain, read_noise) {
        (Some(g), Some(rn)) if g > f32::EPSILON => {
            // Full CCD noise model: shot + sky + read noise
            flux / g + npix * (sky_var + (rn * rn) / (g * g))
        }
        (Some(g), None) if g > f32::EPSILON => {
            // Shot noise + sky noise (no read noise)
            flux / g + npix * sky_var
        }
        _ => {
            // Background-dominated: npix × sky_var
            npix * sky_var
        }
    };

    if total_var > f32::EPSILON {
        flux / total_var.sqrt()
    } else {
        flux / f32::EPSILON
    }
}
