//! Sub-pixel centroid computation and star quality metrics.
//!
//! Uses iterative weighted centroid algorithm for sub-pixel accurate positioning,
//! typically achieving ~0.05 pixel accuracy.
//!
//! Also provides 2D Gaussian and Moffat profile fitting for higher precision
//! centroid computation (~0.01 pixel accuracy).

pub mod gaussian_fit;
pub mod linear_solver;
pub mod moffat_fit;

#[cfg(test)]
mod tests;

#[cfg(feature = "bench")]
pub mod bench;

// Re-export fitting functions and types for convenience
pub use gaussian_fit::{GaussianFitConfig, GaussianFitResult, fit_gaussian_2d};
#[allow(unused_imports)]
pub use gaussian_fit::{fwhm_to_sigma, sigma_to_fwhm};
pub use moffat_fit::{MoffatFitConfig, MoffatFitResult, fit_moffat_2d};
#[allow(unused_imports)]
pub use moffat_fit::{alpha_beta_to_fwhm, fwhm_beta_to_alpha};

use super::background::BackgroundMap;
use super::cosmic_ray::compute_laplacian_snr;
use super::detection::StarCandidate;
use super::{Star, StarDetectionConfig};

/// Minimum stamp radius for centroid computation.
const MIN_STAMP_RADIUS: usize = 4;

/// Maximum stamp radius for centroid computation.
const MAX_STAMP_RADIUS: usize = 15;

/// Maximum iterations for centroid refinement.
pub(crate) const MAX_ITERATIONS: usize = 10;

/// Convergence threshold in pixels squared.
pub(crate) const CONVERGENCE_THRESHOLD_SQ: f32 = 0.001 * 0.001;

/// Compute adaptive stamp radius based on expected FWHM.
///
/// The stamp should be large enough to capture most of the PSF flux (~3.5× FWHM)
/// while not being so large that it includes too much noise or neighboring stars.
#[inline]
pub(crate) fn compute_stamp_radius(expected_fwhm: f32) -> usize {
    // Use ~3.5× FWHM to capture >99% of Gaussian PSF flux
    // Clamp to reasonable bounds
    let radius = (expected_fwhm * 1.75).ceil() as usize;
    radius.clamp(MIN_STAMP_RADIUS, MAX_STAMP_RADIUS)
}

/// Check if position is within valid bounds for stamp extraction.
#[inline]
pub(crate) fn is_valid_stamp_position(
    cx: f32,
    cy: f32,
    width: usize,
    height: usize,
    stamp_radius: usize,
) -> bool {
    let icx = cx.round() as isize;
    let icy = cy.round() as isize;
    icx >= stamp_radius as isize
        && icy >= stamp_radius as isize
        && icx < (width - stamp_radius) as isize
        && icy < (height - stamp_radius) as isize
}

/// Compute sub-pixel centroid and quality metrics for a star candidate.
///
/// Returns `None` if the candidate fails quality checks during centroid computation.
///
/// # Algorithm
///
/// Uses iterative weighted centroid:
/// 1. Start with peak pixel as initial guess
/// 2. Extract stamp centered on current position (size adapts to expected FWHM)
/// 3. Compute weighted centroid with Gaussian weights
/// 4. Repeat until convergence (~0.05 pixel accuracy)
/// 5. Compute quality metrics (FWHM, SNR, eccentricity)
pub fn compute_centroid(
    pixels: &[f32],
    width: usize,
    height: usize,
    background: &BackgroundMap,
    candidate: &StarCandidate,
    config: &StarDetectionConfig,
) -> Option<Star> {
    // Compute adaptive stamp radius based on expected FWHM
    let stamp_radius = compute_stamp_radius(config.expected_fwhm);

    // Initial position from peak
    let mut cx = candidate.peak_x as f32;
    let mut cy = candidate.peak_y as f32;

    // Iterative centroid refinement
    for _ in 0..MAX_ITERATIONS {
        let (new_cx, new_cy) =
            refine_centroid(pixels, width, height, background, cx, cy, stamp_radius)?;

        let dx = new_cx - cx;
        let dy = new_cy - cy;
        cx = new_cx;
        cy = new_cy;

        if dx * dx + dy * dy < CONVERGENCE_THRESHOLD_SQ {
            break;
        }
    }

    // Compute quality metrics
    let metrics = compute_metrics(pixels, width, height, background, cx, cy, stamp_radius)?;

    // Compute L.A.Cosmic Laplacian SNR for cosmic ray detection
    let icx = cx.round() as isize;
    let icy = cy.round() as isize;
    let idx = icy as usize * width + icx as usize;
    let local_bg = background.background[idx];
    let local_noise = background.noise[idx];
    let laplacian_snr_value = compute_laplacian_snr(
        pixels,
        width,
        height,
        cx,
        cy,
        stamp_radius,
        local_bg,
        local_noise,
    );

    Some(Star {
        x: cx,
        y: cy,
        flux: metrics.flux,
        fwhm: metrics.fwhm,
        eccentricity: metrics.eccentricity,
        snr: metrics.snr,
        peak: candidate.peak_value,
        sharpness: metrics.sharpness,
        roundness1: metrics.roundness1,
        roundness2: metrics.roundness2,
        laplacian_snr: laplacian_snr_value,
    })
}

/// Single iteration of centroid refinement.
///
/// Returns (new_x, new_y) or None if position is invalid.
pub(crate) fn refine_centroid(
    pixels: &[f32],
    width: usize,
    height: usize,
    background: &BackgroundMap,
    cx: f32,
    cy: f32,
    stamp_radius: usize,
) -> Option<(f32, f32)> {
    if !is_valid_stamp_position(cx, cy, width, height, stamp_radius) {
        return None;
    }

    let icx = cx.round() as isize;
    let icy = cy.round() as isize;

    let sigma = 2.0f32; // Gaussian weight sigma
    let two_sigma_sq = 2.0 * sigma * sigma;

    let mut sum_x = 0.0f32;
    let mut sum_y = 0.0f32;
    let mut sum_w = 0.0f32;

    let stamp_radius_i32 = stamp_radius as i32;
    for dy in -stamp_radius_i32..=stamp_radius_i32 {
        for dx in -stamp_radius_i32..=stamp_radius_i32 {
            let x = (icx + dx as isize) as usize;
            let y = (icy + dy as isize) as usize;
            let idx = y * width + x;

            // Background-subtracted value
            let value = (pixels[idx] - background.background[idx]).max(0.0);

            // Gaussian weight based on distance from current centroid
            let fx = x as f32 - cx;
            let fy = y as f32 - cy;
            let dist_sq = fx * fx + fy * fy;
            let weight = value * (-dist_sq / two_sigma_sq).exp();

            sum_x += x as f32 * weight;
            sum_y += y as f32 * weight;
            sum_w += weight;
        }
    }

    if sum_w < f32::EPSILON {
        return None;
    }

    let new_cx = sum_x / sum_w;
    let new_cy = sum_y / sum_w;

    // Reject if centroid moved too far (likely bad detection)
    let stamp_size = 2 * stamp_radius + 1;
    let max_move = stamp_size as f32 / 4.0;
    if (new_cx - cx).abs() > max_move || (new_cy - cy).abs() > max_move {
        return None;
    }

    Some((new_cx, new_cy))
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
pub(crate) fn compute_metrics(
    pixels: &[f32],
    width: usize,
    height: usize,
    background: &BackgroundMap,
    cx: f32,
    cy: f32,
    stamp_radius: usize,
) -> Option<StarMetrics> {
    if !is_valid_stamp_position(cx, cy, width, height, stamp_radius) {
        return None;
    }

    let icx = cx.round() as isize;
    let icy = cy.round() as isize;

    // Collect background-subtracted values and positions
    let mut flux = 0.0f32;
    let mut core_flux = 0.0f32; // Flux in 3x3 core for sharpness calculation
    let mut sum_r2 = 0.0f32;
    let mut sum_x2 = 0.0f32;
    let mut sum_y2 = 0.0f32;
    let mut sum_xy = 0.0f32;
    let mut noise_sum = 0.0f32;
    let mut noise_count = 0usize;
    let mut peak_value = 0.0f32;

    // For roundness calculation: marginal sums
    let stamp_size = 2 * stamp_radius + 1;
    let mut marginal_x = vec![0.0f32; stamp_size];
    let mut marginal_y = vec![0.0f32; stamp_size];

    let stamp_radius_i32 = stamp_radius as i32;
    let outer_ring_threshold = (stamp_radius_i32 - 2) * (stamp_radius_i32 - 2);
    for dy in -stamp_radius_i32..=stamp_radius_i32 {
        for dx in -stamp_radius_i32..=stamp_radius_i32 {
            let x = (icx + dx as isize) as usize;
            let y = (icy + dy as isize) as usize;
            let idx = y * width + x;

            let bg = background.background[idx];
            let value = (pixels[idx] - bg).max(0.0);

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
            let fx = x as f32 - cx;
            let fy = y as f32 - cy;
            sum_r2 += value * (fx * fx + fy * fy);
            sum_x2 += value * fx * fx;
            sum_y2 += value * fy * fy;
            sum_xy += value * fx * fy;

            // Collect noise from background region (outer ring)
            let r2 = dx * dx + dy * dy;
            if r2 > outer_ring_threshold {
                noise_sum += background.noise[idx];
                noise_count += 1;
            }
        }
    }

    if flux < f32::EPSILON {
        return None;
    }

    // FWHM from second moment (assuming Gaussian PSF)
    // For 2D Gaussian: E[r^2] = E[x^2 + y^2] = 2*sigma^2
    // So sigma^2 = sum(r^2 * I) / sum(I) / 2
    // FWHM = 2.355 * sigma
    let sigma_sq = sum_r2 / flux / 2.0;
    let fwhm = 2.355 * sigma_sq.sqrt();

    // Eccentricity from covariance matrix
    // eigenvalues of [[sum_x2, sum_xy], [sum_xy, sum_y2]] / flux
    let cxx = sum_x2 / flux;
    let cyy = sum_y2 / flux;
    let cxy = sum_xy / flux;

    let trace = cxx + cyy;
    let det = cxx * cyy - cxy * cxy;
    let discriminant = (trace * trace - 4.0 * det).max(0.0);
    let lambda1 = (trace + discriminant.sqrt()) / 2.0;
    let lambda2 = (trace - discriminant.sqrt()) / 2.0;

    let eccentricity = if lambda1 > f32::EPSILON {
        (1.0 - lambda2 / lambda1).sqrt().clamp(0.0, 1.0)
    } else {
        0.0
    };

    // SNR = flux / (noise * sqrt(aperture_area))
    let avg_noise = if noise_count > 0 {
        noise_sum / noise_count as f32
    } else {
        background.noise[icy as usize * width + icx as usize]
    };

    let aperture_area = (2 * stamp_radius + 1).pow(2) as f32;
    let snr = if avg_noise > f32::EPSILON {
        flux / (avg_noise * aperture_area.sqrt())
    } else {
        flux / f32::EPSILON
    };

    // Sharpness = peak / core_flux
    // Cosmic rays: most flux in single pixel -> sharpness ~= 1/9 to 1.0
    // Real stars: flux spread across PSF -> sharpness ~= 0.2-0.5
    let sharpness = if core_flux > f32::EPSILON {
        (peak_value / core_flux).clamp(0.0, 1.0)
    } else {
        1.0 // No core flux means very sharp (likely artifact)
    };

    // Compute roundness metrics (DAOFIND style)
    let (roundness1, roundness2) = compute_roundness(
        &marginal_x,
        &marginal_y,
        stamp_radius,
        pixels,
        width,
        icx,
        icy,
    );

    Some(StarMetrics {
        flux,
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
/// - GROUND: (Hx - Hy) / (Hx + Hy) where Hx, Hy are heights of marginal Gaussian fits
/// - SROUND: Symmetry-based roundness measuring bilateral vs four-fold symmetry
fn compute_roundness(
    marginal_x: &[f32],
    marginal_y: &[f32],
    stamp_radius: usize,
    pixels: &[f32],
    width: usize,
    icx: isize,
    icy: isize,
) -> (f32, f32) {
    // GROUND: Compare heights of marginal distributions
    // The "height" is the peak of the marginal distribution
    let hx = marginal_x.iter().fold(0.0f32, |a, &b| a.max(b));
    let hy = marginal_y.iter().fold(0.0f32, |a, &b| a.max(b));

    let roundness1 = if hx + hy > f32::EPSILON {
        (hx - hy) / (hx + hy)
    } else {
        0.0
    };

    // SROUND: Symmetry-based roundness
    // Compare sum of pixels on opposite sides of center
    // A symmetric source should have equal flux on all sides
    let stamp_radius_i32 = stamp_radius as i32;
    let mut sum_left = 0.0f32;
    let mut sum_right = 0.0f32;
    let mut sum_top = 0.0f32;
    let mut sum_bottom = 0.0f32;

    for dy in -stamp_radius_i32..=stamp_radius_i32 {
        for dx in -stamp_radius_i32..=stamp_radius_i32 {
            if dx == 0 && dy == 0 {
                continue;
            }
            let x = (icx + dx as isize) as usize;
            let y = (icy + dy as isize) as usize;
            let idx = y * width + x;
            let value = pixels[idx];

            if dx < 0 {
                sum_left += value;
            } else if dx > 0 {
                sum_right += value;
            }
            if dy < 0 {
                sum_top += value;
            } else if dy > 0 {
                sum_bottom += value;
            }
        }
    }

    // Compute asymmetry in x and y directions
    let total_x = sum_left + sum_right;
    let total_y = sum_top + sum_bottom;

    let asym_x = if total_x > f32::EPSILON {
        (sum_right - sum_left) / total_x
    } else {
        0.0
    };

    let asym_y = if total_y > f32::EPSILON {
        (sum_bottom - sum_top) / total_y
    } else {
        0.0
    };

    // SROUND is the RMS asymmetry
    let roundness2 = (asym_x * asym_x + asym_y * asym_y).sqrt();

    (roundness1.clamp(-1.0, 1.0), roundness2.clamp(0.0, 1.0))
}
