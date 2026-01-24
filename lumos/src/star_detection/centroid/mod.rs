//! Sub-pixel centroid computation and star quality metrics.
//!
//! Uses iterative weighted centroid algorithm for sub-pixel accurate positioning,
//! typically achieving ~0.05 pixel accuracy.

#[cfg(test)]
mod tests;

use super::background::BackgroundMap;
use super::detection::StarCandidate;
use super::{Star, StarDetectionConfig};

/// Stamp size for centroid computation (pixels on each side of center).
const STAMP_RADIUS: usize = 7;

/// Maximum iterations for centroid refinement.
const MAX_ITERATIONS: usize = 10;

/// Convergence threshold in pixels.
const CONVERGENCE_THRESHOLD: f32 = 0.001;

/// Compute sub-pixel centroid and quality metrics for a star candidate.
///
/// Returns `None` if the candidate fails quality checks during centroid computation.
///
/// # Algorithm
///
/// Uses iterative weighted centroid:
/// 1. Start with peak pixel as initial guess
/// 2. Extract stamp centered on current position
/// 3. Compute weighted centroid with Gaussian weights
/// 4. Repeat until convergence (~0.05 pixel accuracy)
/// 5. Compute quality metrics (FWHM, SNR, eccentricity)
pub fn compute_centroid(
    pixels: &[f32],
    width: usize,
    height: usize,
    background: &BackgroundMap,
    candidate: &StarCandidate,
    _config: &StarDetectionConfig,
) -> Option<Star> {
    // Initial position from peak
    let mut cx = candidate.peak_x as f32;
    let mut cy = candidate.peak_y as f32;

    // Iterative centroid refinement
    for _ in 0..MAX_ITERATIONS {
        let (new_cx, new_cy, converged) =
            refine_centroid(pixels, width, height, background, cx, cy)?;

        let dx = new_cx - cx;
        let dy = new_cy - cy;
        cx = new_cx;
        cy = new_cy;

        if converged || (dx * dx + dy * dy) < CONVERGENCE_THRESHOLD * CONVERGENCE_THRESHOLD {
            break;
        }
    }

    // Compute quality metrics
    let metrics = compute_metrics(pixels, width, height, background, cx, cy)?;

    Some(Star {
        x: cx,
        y: cy,
        flux: metrics.flux,
        fwhm: metrics.fwhm,
        eccentricity: metrics.eccentricity,
        snr: metrics.snr,
        peak: candidate.peak_value,
    })
}

/// Single iteration of centroid refinement.
///
/// Returns (new_x, new_y, converged).
fn refine_centroid(
    pixels: &[f32],
    width: usize,
    height: usize,
    background: &BackgroundMap,
    cx: f32,
    cy: f32,
) -> Option<(f32, f32, bool)> {
    let icx = cx.round() as isize;
    let icy = cy.round() as isize;

    // Check bounds
    if icx < STAMP_RADIUS as isize
        || icy < STAMP_RADIUS as isize
        || icx >= (width - STAMP_RADIUS) as isize
        || icy >= (height - STAMP_RADIUS) as isize
    {
        return None;
    }

    let stamp_size = 2 * STAMP_RADIUS + 1;
    let sigma = 2.0f32; // Gaussian weight sigma

    let mut sum_x = 0.0f32;
    let mut sum_y = 0.0f32;
    let mut sum_w = 0.0f32;

    for dy in -(STAMP_RADIUS as i32)..=(STAMP_RADIUS as i32) {
        for dx in -(STAMP_RADIUS as i32)..=(STAMP_RADIUS as i32) {
            let x = (icx + dx as isize) as usize;
            let y = (icy + dy as isize) as usize;
            let idx = y * width + x;

            // Background-subtracted value
            let value = (pixels[idx] - background.background[idx]).max(0.0);

            // Gaussian weight based on distance from current centroid
            let fx = x as f32 - cx;
            let fy = y as f32 - cy;
            let dist_sq = fx * fx + fy * fy;
            let weight = value * (-dist_sq / (2.0 * sigma * sigma)).exp();

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

    // Check if centroid moved outside stamp
    let moved_far = (new_cx - cx).abs() > stamp_size as f32 / 4.0
        || (new_cy - cy).abs() > stamp_size as f32 / 4.0;

    Some((new_cx, new_cy, !moved_far))
}

/// Quality metrics for a star.
struct StarMetrics {
    flux: f32,
    fwhm: f32,
    eccentricity: f32,
    snr: f32,
}

/// Compute quality metrics for a star at the given position.
fn compute_metrics(
    pixels: &[f32],
    width: usize,
    height: usize,
    background: &BackgroundMap,
    cx: f32,
    cy: f32,
) -> Option<StarMetrics> {
    let icx = cx.round() as isize;
    let icy = cy.round() as isize;

    // Check bounds
    if icx < STAMP_RADIUS as isize
        || icy < STAMP_RADIUS as isize
        || icx >= (width - STAMP_RADIUS) as isize
        || icy >= (height - STAMP_RADIUS) as isize
    {
        return None;
    }

    // Collect background-subtracted values and positions
    let mut flux = 0.0f32;
    let mut sum_r2 = 0.0f32;
    let mut sum_x2 = 0.0f32;
    let mut sum_y2 = 0.0f32;
    let mut sum_xy = 0.0f32;
    let mut noise_sum = 0.0f32;
    let mut noise_count = 0usize;
    let mut peak_value = 0.0f32;

    for dy in -(STAMP_RADIUS as i32)..=(STAMP_RADIUS as i32) {
        for dx in -(STAMP_RADIUS as i32)..=(STAMP_RADIUS as i32) {
            let x = (icx + dx as isize) as usize;
            let y = (icy + dy as isize) as usize;
            let idx = y * width + x;

            let bg = background.background[idx];
            let value = (pixels[idx] - bg).max(0.0);

            flux += value;
            peak_value = peak_value.max(value);

            // Weighted second moments for FWHM and eccentricity
            let fx = x as f32 - cx;
            let fy = y as f32 - cy;
            sum_r2 += value * (fx * fx + fy * fy);
            sum_x2 += value * fx * fx;
            sum_y2 += value * fy * fy;
            sum_xy += value * fx * fy;

            // Collect noise from background region (outer ring)
            let r2 = dx * dx + dy * dy;
            if r2 > (STAMP_RADIUS as i32 - 2) * (STAMP_RADIUS as i32 - 2) {
                noise_sum += background.noise[idx];
                noise_count += 1;
            }
        }
    }

    if flux < f32::EPSILON {
        return None;
    }

    // FWHM from second moment (assuming Gaussian PSF)
    // For Gaussian: sigma^2 = sum(r^2 * I) / sum(I)
    // FWHM = 2.355 * sigma
    let sigma_sq = sum_r2 / flux;
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

    let aperture_area = (2 * STAMP_RADIUS + 1).pow(2) as f32;
    let snr = if avg_noise > f32::EPSILON {
        flux / (avg_noise * aperture_area.sqrt())
    } else {
        flux / f32::EPSILON
    };

    Some(StarMetrics {
        flux,
        fwhm,
        eccentricity,
        snr,
    })
}
