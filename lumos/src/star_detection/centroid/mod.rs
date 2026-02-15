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

// Re-export fitting functions and types for convenience
pub use gaussian_fit::{GaussianFitConfig, GaussianFitResult, fit_gaussian_2d};
pub use moffat_fit::{MoffatFitConfig, MoffatFitResult, fit_moffat_2d};
pub use moffat_fit::{alpha_beta_to_fwhm, fwhm_beta_to_alpha};

use arrayvec::ArrayVec;
use glam::{DVec2, Vec2};

use super::background::BackgroundEstimate;
use super::config::Config;
use super::deblend::Region;
use super::{CentroidMethod, LocalBackgroundMethod, Star};
use crate::common::Buffer2;
use crate::math::FWHM_TO_SIGMA;

// =============================================================================
// Stamp and Centroid Constants
// =============================================================================

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
/// Tuple of (background, noise) or None if not enough valid pixels
fn compute_annulus_background(
    pixels: &[f32],
    width: usize,
    height: usize,
    pos: Vec2,
    inner_radius: usize,
    outer_radius: usize,
) -> Option<(f32, f32)> {
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
    let (median, sigma) = sigma_clipped_median_mad(&mut values, 3.0, 2);
    Some((median, sigma))
}

/// Compute sigma-clipped median and MAD using the shared implementation.
/// Uses stack-allocated ArrayVec for deviations to avoid heap allocation.
#[inline]
fn sigma_clipped_median_mad(values: &mut [f32], kappa: f32, iterations: usize) -> (f32, f32) {
    let mut deviations: ArrayVec<f32, MAX_ANNULUS_PIXELS> = ArrayVec::new();
    // Resize to match values length
    deviations.extend(std::iter::repeat_n(0.0, values.len()));
    crate::math::sigma_clipped_median_mad_arrayvec(values, &mut deviations, kappa, iterations)
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
    config: &Config,
) -> Option<Star> {
    let width = pixels.width();
    let height = pixels.height();
    // Compute adaptive stamp radius based on expected FWHM
    let stamp_radius = compute_stamp_radius(config.expected_fwhm);

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
            config.expected_fwhm,
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
    let global_fallback = || {
        (
            background.background.row(bg_y)[bg_x],
            background.noise.row(bg_y)[bg_x],
        )
    };

    let (local_bg, _local_noise) = match config.local_background {
        LocalBackgroundMethod::GlobalMap => global_fallback(),
        LocalBackgroundMethod::LocalAnnulus => {
            let inner_radius = stamp_radius;
            let outer_radius = (stamp_radius as f32 * 1.5).ceil() as usize;
            compute_annulus_background(pixels, width, height, pos, inner_radius, outer_radius)
                .unwrap_or_else(global_fallback)
        }
    };

    // Refine with profile fitting if requested.
    // When fit converges, also extract FWHM and eccentricity from fit parameters
    // (more accurate than moment-based estimates).
    let mut fit_fwhm: Option<f32> = None;
    let mut fit_eccentricity: Option<f32> = None;

    match config.centroid_method {
        CentroidMethod::GaussianFit => {
            let fit_config = GaussianFitConfig {
                position_convergence_threshold: CENTROID_CONVERGENCE_THRESHOLD as f64,
                ..GaussianFitConfig::default()
            };
            if let Some(result) = fit_gaussian_2d(pixels, pos, stamp_radius, local_bg, &fit_config)
                .filter(|r| r.converged)
            {
                pos = result.pos;
                // FWHM from geometric mean of sigma_x, sigma_y
                let geo_sigma = (result.sigma.x * result.sigma.y).sqrt();
                fit_fwhm = Some(crate::math::sigma_to_fwhm(geo_sigma));
                // Eccentricity from sigma ratio: e = sqrt(1 - min/max)
                let (s_min, s_max) = if result.sigma.x < result.sigma.y {
                    (result.sigma.x, result.sigma.y)
                } else {
                    (result.sigma.y, result.sigma.x)
                };
                if s_max > f32::EPSILON {
                    fit_eccentricity = Some((1.0 - s_min / s_max).sqrt().clamp(0.0, 1.0));
                }
            }
        }
        CentroidMethod::MoffatFit { beta } => {
            let fit_config = MoffatFitConfig {
                fit_beta: false,
                fixed_beta: beta,
                lm: lm_optimizer::LMConfig {
                    position_convergence_threshold: CENTROID_CONVERGENCE_THRESHOLD as f64,
                    ..lm_optimizer::LMConfig::default()
                },
            };
            if let Some(result) = fit_moffat_2d(pixels, pos, stamp_radius, local_bg, &fit_config)
                .filter(|r| r.converged)
            {
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

    let mut metrics = compute_metrics(pixels, background, pos, stamp_radius, gain, read_noise)?;

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
pub(crate) fn compute_metrics(
    pixels: &Buffer2<f32>,
    background: &BackgroundEstimate,
    pos: Vec2,
    stamp_radius: usize,
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

            let bg = bg_row[x];
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
            if r2 > outer_ring_threshold {
                noise_sum += noise_row[x] as f64;
                noise_count += 1;
            }
        }
    }

    if flux < f64::EPSILON {
        return None;
    }

    // FWHM from second moment (assuming Gaussian PSF)
    let sigma_sq = sum_r2 / flux / 2.0;
    let fwhm = crate::math::sigma_to_fwhm(sigma_sq.sqrt() as f32);

    // Eccentricity from covariance matrix
    let cxx = sum_x2 / flux;
    let cyy = sum_y2 / flux;
    let cxy = sum_xy / flux;

    let trace = cxx + cyy;
    let det = cxx * cyy - cxy * cxy;
    let discriminant = (trace * trace - 4.0 * det).max(0.0);
    let lambda1 = (trace + discriminant.sqrt()) / 2.0;
    let lambda2 = (trace - discriminant.sqrt()) / 2.0;

    let eccentricity = if lambda1 > f64::EPSILON {
        (1.0 - lambda2 / lambda1).sqrt().clamp(0.0, 1.0) as f32
    } else {
        0.0
    };

    // Compute SNR using appropriate noise model
    let avg_noise = if noise_count > 0 {
        (noise_sum / noise_count as f64) as f32
    } else {
        background.noise.row(icy as usize)[icx as usize]
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
