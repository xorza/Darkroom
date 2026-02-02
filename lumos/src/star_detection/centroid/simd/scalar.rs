//! Scalar (non-SIMD) implementation of centroid refinement.

use crate::math::{FWHM_TO_SIGMA, fast_exp};
use crate::star_detection::background::BackgroundMap;
use crate::star_detection::centroid::is_valid_stamp_position;

/// Scalar fallback for centroid refinement.
#[allow(clippy::too_many_arguments)]
pub fn refine_centroid_scalar(
    pixels: &[f32],
    width: usize,
    height: usize,
    background: &BackgroundMap,
    cx: f32,
    cy: f32,
    stamp_radius: usize,
    expected_fwhm: f32,
) -> Option<(f32, f32)> {
    if !is_valid_stamp_position(cx, cy, width, height, stamp_radius) {
        return None;
    }

    let icx = cx.round() as isize;
    let icy = cy.round() as isize;

    // Adaptive sigma based on expected FWHM
    // sigma ≈ FWHM / FWHM_TO_SIGMA, use 0.8× for tighter weighting to reduce noise
    let sigma = (expected_fwhm / FWHM_TO_SIGMA * 0.8).clamp(1.0, stamp_radius as f32 * 0.5);
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
            // Using fast_exp approximation (~4% max error) for performance
            let fx = x as f32 - cx;
            let fy = y as f32 - cy;
            let dist_sq = fx * fx + fy * fy;
            let weight = value * fast_exp(-dist_sq / two_sigma_sq);

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
