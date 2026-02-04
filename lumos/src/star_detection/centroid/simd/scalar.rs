//! Scalar (non-SIMD) implementation of centroid refinement.

use glam::Vec2;

use crate::math::{FWHM_TO_SIGMA, fast_exp};
use crate::star_detection::centroid::is_valid_stamp_position;
use crate::star_detection::image_stats::ImageStats;

/// Scalar fallback for centroid refinement.
pub fn refine_centroid_scalar(
    pixels: &[f32],
    width: usize,
    height: usize,
    background: &ImageStats,
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
    let two_sigma_sq = 2.0 * sigma * sigma;

    let mut sum_pos = Vec2::ZERO;
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
            let pixel_pos = Vec2::new(x as f32, y as f32);
            let dist_sq = (pixel_pos - pos).length_squared();
            let weight = value * fast_exp(-dist_sq / two_sigma_sq);

            sum_pos += pixel_pos * weight;
            sum_w += weight;
        }
    }

    if sum_w < f32::EPSILON {
        return None;
    }

    let new_pos = sum_pos / sum_w;

    // Reject if centroid moved too far (likely bad detection)
    let stamp_size = 2 * stamp_radius + 1;
    let max_move = stamp_size as f32 / 4.0;
    if (new_pos - pos).abs().max_element() > max_move {
        return None;
    }

    Some(new_pos)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::Buffer2;
    use crate::star_detection::config::Config;
    use glam::Vec2;

    /// Create a synthetic Gaussian star for testing.
    fn make_gaussian_star(
        width: usize,
        height: usize,
        pos: Vec2,
        sigma: f32,
        amplitude: f32,
        background: f32,
    ) -> Buffer2<f32> {
        let mut pixels = vec![background; width * height];
        for y in 0..height {
            for x in 0..width {
                let pixel_pos = Vec2::new(x as f32, y as f32);
                let r2 = pixel_pos.distance_squared(pos);
                let value = amplitude * (-r2 / (2.0 * sigma * sigma)).exp();
                if value > 0.001 {
                    pixels[y * width + x] += value;
                }
            }
        }
        Buffer2::new(width, height, pixels)
    }

    fn make_uniform_background(width: usize, height: usize, value: f32) -> Buffer2<f32> {
        Buffer2::new(width, height, vec![value; width * height])
    }

    #[test]
    fn test_centered_star() {
        let width = 64;
        let height = 64;
        let pixels = make_gaussian_star(width, height, Vec2::splat(32.0), 2.5, 0.8, 0.1);
        let bg = crate::testing::estimate_background(&pixels, &Config::default());

        let result = refine_centroid_scalar(&pixels, width, height, &bg, Vec2::splat(32.0), 7, 4.0);

        assert!(result.is_some());
        let pos = result.unwrap();
        assert!(
            (pos.x - 32.0).abs() < 0.1,
            "cx={} should be close to 32.0",
            pos.x
        );
        assert!(
            (pos.y - 32.0).abs() < 0.1,
            "cy={} should be close to 32.0",
            pos.y
        );
    }

    #[test]
    fn test_offset_star() {
        let width = 64;
        let height = 64;
        let true_pos = Vec2::new(32.3, 32.7);
        let pixels = make_gaussian_star(width, height, true_pos, 2.5, 0.8, 0.1);
        let bg = crate::testing::estimate_background(&pixels, &Config::default());

        let result = refine_centroid_scalar(&pixels, width, height, &bg, Vec2::splat(32.0), 7, 4.0);

        assert!(result.is_some());
        let pos = result.unwrap();
        // Single iteration moves toward true center but may not fully converge
        assert!(
            (pos.x - 32.0).abs() < 0.5,
            "cx={} should have moved toward {}",
            pos.x,
            true_pos.x
        );
        assert!(
            (pos.y - 32.0).abs() < 0.5,
            "cy={} should have moved toward {}",
            pos.y,
            true_pos.y
        );
    }

    #[test]
    fn test_invalid_position_left_edge() {
        let width = 64;
        let height = 64;
        let pixels = make_gaussian_star(width, height, Vec2::splat(32.0), 2.5, 0.8, 0.1);
        let bg = crate::testing::estimate_background(&pixels, &Config::default());

        let result =
            refine_centroid_scalar(&pixels, width, height, &bg, Vec2::new(3.0, 32.0), 7, 4.0);
        assert!(result.is_none());
    }

    #[test]
    fn test_invalid_position_right_edge() {
        let width = 64;
        let height = 64;
        let pixels = make_gaussian_star(width, height, Vec2::splat(32.0), 2.5, 0.8, 0.1);
        let bg = crate::testing::estimate_background(&pixels, &Config::default());

        let result =
            refine_centroid_scalar(&pixels, width, height, &bg, Vec2::new(61.0, 32.0), 7, 4.0);
        assert!(result.is_none());
    }

    #[test]
    fn test_invalid_position_top_edge() {
        let width = 64;
        let height = 64;
        let pixels = make_gaussian_star(width, height, Vec2::splat(32.0), 2.5, 0.8, 0.1);
        let bg = crate::testing::estimate_background(&pixels, &Config::default());

        let result =
            refine_centroid_scalar(&pixels, width, height, &bg, Vec2::new(32.0, 3.0), 7, 4.0);
        assert!(result.is_none());
    }

    #[test]
    fn test_invalid_position_bottom_edge() {
        let width = 64;
        let height = 64;
        let pixels = make_gaussian_star(width, height, Vec2::splat(32.0), 2.5, 0.8, 0.1);
        let bg = crate::testing::estimate_background(&pixels, &Config::default());

        let result =
            refine_centroid_scalar(&pixels, width, height, &bg, Vec2::new(32.0, 61.0), 7, 4.0);
        assert!(result.is_none());
    }

    #[test]
    fn test_zero_signal() {
        let width = 64;
        let height = 64;
        let pixels = make_uniform_background(width, height, 0.1);
        let bg = crate::testing::estimate_background(&pixels, &Config::default());

        let result = refine_centroid_scalar(&pixels, width, height, &bg, Vec2::splat(32.0), 7, 4.0);
        assert!(result.is_none(), "Should return None for zero signal");
    }

    #[test]
    fn test_different_stamp_radii() {
        let width = 128;
        let height = 128;
        let pixels = make_gaussian_star(width, height, Vec2::new(64.3, 64.7), 4.0, 0.8, 0.1);
        let bg = crate::testing::estimate_background(&pixels, &Config::default());

        // Test with small stamp (9x9)
        let result_small =
            refine_centroid_scalar(&pixels, width, height, &bg, Vec2::splat(64.0), 4, 6.0);
        assert!(result_small.is_some());

        // Test with medium stamp (15x15)
        let result_med =
            refine_centroid_scalar(&pixels, width, height, &bg, Vec2::splat(64.0), 7, 6.0);
        assert!(result_med.is_some());

        // Test with large stamp (21x21)
        let result_large =
            refine_centroid_scalar(&pixels, width, height, &bg, Vec2::splat(64.0), 10, 6.0);
        assert!(result_large.is_some());

        // All should produce reasonable results
        for (name, result) in [
            ("small", result_small),
            ("medium", result_med),
            ("large", result_large),
        ] {
            let pos = result.unwrap();
            assert!(
                (pos.x - 64.0).abs() < 0.5,
                "{name}: cx={} should be reasonable",
                pos.x
            );
            assert!(
                (pos.y - 64.0).abs() < 0.5,
                "{name}: cy={} should be reasonable",
                pos.y
            );
        }
    }

    #[test]
    fn test_centroid_moves_too_far() {
        let width = 64;
        let height = 64;
        // Star at (45, 45) but we start search at (32, 32) - too far
        let pixels = make_gaussian_star(width, height, Vec2::splat(45.0), 2.5, 0.8, 0.1);
        let bg = crate::testing::estimate_background(&pixels, &Config::default());

        // With stamp_radius=7, max_move = 15/4 = 3.75 pixels
        let result = refine_centroid_scalar(&pixels, width, height, &bg, Vec2::splat(32.0), 7, 4.0);
        assert!(
            result.is_none(),
            "Should reject when centroid moves too far"
        );
    }

    #[test]
    fn test_negative_background_subtraction() {
        let width = 64;
        let height = 64;
        // Create image where background is higher than star in some areas
        let mut pixels = vec![0.5f32; width * height];
        for y in 28..36 {
            for x in 28..36 {
                let pixel_pos = Vec2::new(x as f32, y as f32);
                let r2 = pixel_pos.distance_squared(Vec2::splat(32.0));
                pixels[y * width + x] += 0.8 * (-r2 / 8.0).exp();
            }
        }
        let pixels = Buffer2::new(width, height, pixels);
        let bg = crate::testing::estimate_background(&pixels, &Config::default());

        // Should still work - negative values clamped to 0
        let result = refine_centroid_scalar(&pixels, width, height, &bg, Vec2::splat(32.0), 7, 4.0);
        assert!(result.is_some());
    }

    #[test]
    fn test_very_bright_star() {
        let width = 64;
        let height = 64;
        let pixels = make_gaussian_star(width, height, Vec2::new(32.3, 32.7), 2.5, 10000.0, 0.1);
        let bg = crate::testing::estimate_background(&pixels, &Config::default());

        let result = refine_centroid_scalar(&pixels, width, height, &bg, Vec2::splat(32.0), 7, 4.0);
        assert!(result.is_some());
        let pos = result.unwrap();
        assert!((pos.x - 32.0).abs() < 0.5);
        assert!((pos.y - 32.0).abs() < 0.5);
    }

    #[test]
    fn test_very_faint_star() {
        let width = 64;
        let height = 64;
        let pixels = make_gaussian_star(width, height, Vec2::new(32.3, 32.7), 2.5, 0.01, 0.1);
        let bg = crate::testing::estimate_background(&pixels, &Config::default());

        // May or may not succeed - just checking it doesn't crash
        let _ = refine_centroid_scalar(&pixels, width, height, &bg, Vec2::splat(32.0), 7, 4.0);
    }

    #[test]
    fn test_minimum_stamp_radius() {
        let width = 64;
        let height = 64;
        let pixels = make_gaussian_star(width, height, Vec2::splat(32.0), 1.5, 0.8, 0.1);
        let bg = crate::testing::estimate_background(&pixels, &Config::default());

        let result = refine_centroid_scalar(&pixels, width, height, &bg, Vec2::splat(32.0), 4, 2.0);
        assert!(result.is_some());
    }

    #[test]
    fn test_maximum_stamp_radius() {
        let width = 128;
        let height = 128;
        let pixels = make_gaussian_star(width, height, Vec2::splat(64.0), 5.0, 0.8, 0.1);
        let bg = crate::testing::estimate_background(&pixels, &Config::default());

        let result =
            refine_centroid_scalar(&pixels, width, height, &bg, Vec2::splat(64.0), 15, 10.0);
        assert!(result.is_some());
    }
}
