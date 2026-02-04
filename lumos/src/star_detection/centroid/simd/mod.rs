//! SIMD-optimized centroid refinement implementations.
//!
//! This module provides platform-specific SIMD implementations for
//! the centroid refinement inner loop, with automatic runtime dispatch
//! to the best available implementation.

mod scalar;

#[cfg(target_arch = "x86_64")]
mod avx2;
#[cfg(target_arch = "x86_64")]
mod sse;

#[cfg(target_arch = "aarch64")]
mod neon;

#[cfg(test)]
mod bench;

use common::cpu_features;
use glam::Vec2;

use crate::star_detection::image_stats::ImageStats;

pub use scalar::refine_centroid_scalar;

#[cfg(target_arch = "x86_64")]
pub use avx2::refine_centroid_avx2;
#[cfg(target_arch = "x86_64")]
pub use sse::refine_centroid_sse;

#[cfg(target_arch = "aarch64")]
pub use neon::refine_centroid_neon;

/// Refine centroid position using the best available SIMD implementation.
///
/// Automatically dispatches to AVX2, SSE, NEON, or scalar based on
/// runtime CPU feature detection.
pub fn refine_centroid(
    pixels: &[f32],
    width: usize,
    height: usize,
    background: &ImageStats,
    pos: Vec2,
    stamp_radius: usize,
    expected_fwhm: f32,
) -> Option<Vec2> {
    #[cfg(target_arch = "x86_64")]
    {
        if cpu_features::has_avx2_fma() {
            // SAFETY: AVX2 and FMA are available
            return unsafe {
                refine_centroid_avx2(
                    pixels,
                    width,
                    height,
                    background,
                    pos,
                    stamp_radius,
                    expected_fwhm,
                )
            };
        }
        if cpu_features::has_sse4_1() {
            // SAFETY: SSE4.1 is available
            return unsafe {
                refine_centroid_sse(
                    pixels,
                    width,
                    height,
                    background,
                    pos,
                    stamp_radius,
                    expected_fwhm,
                )
            };
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        // NEON is always available on aarch64
        return unsafe {
            refine_centroid_neon(
                pixels,
                width,
                height,
                background,
                pos,
                stamp_radius,
                expected_fwhm,
            )
        };
    }

    refine_centroid_scalar(
        pixels,
        width,
        height,
        background,
        pos,
        stamp_radius,
        expected_fwhm,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::Buffer2;
    use crate::star_detection::config::Config;
    use glam::Vec2;

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
                let dx = x as f32 - pos.x;
                let dy = y as f32 - pos.y;
                let r2 = dx * dx + dy * dy;
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
    fn test_dispatcher_centered_star() {
        let width = 64;
        let height = 64;
        let pixels = make_gaussian_star(width, height, Vec2::splat(32.0), 2.5, 0.8, 0.1);
        let bg = crate::testing::estimate_background(&pixels, &Config::default());

        let result = refine_centroid(&pixels, width, height, &bg, Vec2::splat(32.0), 7, 4.0);

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
    fn test_dispatcher_offset_star() {
        let width = 64;
        let height = 64;
        let true_pos = Vec2::new(32.3, 32.7);
        let pixels = make_gaussian_star(width, height, true_pos, 2.5, 0.8, 0.1);
        let bg = crate::testing::estimate_background(&pixels, &Config::default());

        let result = refine_centroid(&pixels, width, height, &bg, Vec2::splat(32.0), 7, 4.0);

        assert!(result.is_some());
        let pos = result.unwrap();
        // Single iteration moves toward true center but may not reach it
        assert!(
            (pos.x - 32.0).abs() < (true_pos.x - 32.0).abs() + 0.1,
            "cx={} should move toward {}",
            pos.x,
            true_pos.x
        );
        assert!(
            (pos.y - 32.0).abs() < (true_pos.y - 32.0).abs() + 0.1,
            "cy={} should move toward {}",
            pos.y,
            true_pos.y
        );
    }

    #[test]
    fn test_dispatcher_matches_scalar() {
        let width = 64;
        let height = 64;
        let pixels = make_gaussian_star(width, height, Vec2::new(32.3, 32.7), 2.5, 0.8, 0.1);
        let bg = crate::testing::estimate_background(&pixels, &Config::default());

        let scalar =
            refine_centroid_scalar(&pixels, width, height, &bg, Vec2::splat(32.0), 7, 4.0).unwrap();
        let dispatched =
            refine_centroid(&pixels, width, height, &bg, Vec2::splat(32.0), 7, 4.0).unwrap();

        // Should be very close (SIMD uses same algorithm)
        assert!(
            (scalar.x - dispatched.x).abs() < 0.01,
            "Dispatched cx={} should match scalar cx={}",
            dispatched.x,
            scalar.x
        );
        assert!(
            (scalar.y - dispatched.y).abs() < 0.01,
            "Dispatched cy={} should match scalar cy={}",
            dispatched.y,
            scalar.y
        );
    }

    #[test]
    fn test_dispatcher_invalid_position() {
        let width = 64;
        let height = 64;
        let pixels = make_gaussian_star(width, height, Vec2::splat(32.0), 2.5, 0.8, 0.1);
        let bg = crate::testing::estimate_background(&pixels, &Config::default());

        assert!(
            refine_centroid(&pixels, width, height, &bg, Vec2::new(3.0, 32.0), 7, 4.0).is_none()
        );
        assert!(
            refine_centroid(&pixels, width, height, &bg, Vec2::new(61.0, 32.0), 7, 4.0).is_none()
        );
        assert!(
            refine_centroid(&pixels, width, height, &bg, Vec2::new(32.0, 3.0), 7, 4.0).is_none()
        );
        assert!(
            refine_centroid(&pixels, width, height, &bg, Vec2::new(32.0, 61.0), 7, 4.0).is_none()
        );
    }

    #[test]
    fn test_dispatcher_zero_signal() {
        let width = 64;
        let height = 64;
        let pixels = make_uniform_background(width, height, 0.1);
        let bg = crate::testing::estimate_background(&pixels, &Config::default());

        let result = refine_centroid(&pixels, width, height, &bg, Vec2::splat(32.0), 7, 4.0);
        assert!(result.is_none());
    }

    #[test]
    fn test_dispatcher_various_fwhm() {
        let width = 128;
        let height = 128;

        for fwhm in [2.0, 4.0, 6.0, 8.0, 10.0] {
            let sigma = fwhm / 2.355;
            let pixels = make_gaussian_star(width, height, Vec2::new(64.3, 64.7), sigma, 0.8, 0.1);
            let bg = crate::testing::estimate_background(&pixels, &Config::default());

            let stamp_radius = ((fwhm * 1.75).ceil() as usize).clamp(4, 15);

            let result = refine_centroid(
                &pixels,
                width,
                height,
                &bg,
                Vec2::splat(64.0),
                stamp_radius,
                fwhm,
            );

            assert!(
                result.is_some(),
                "FWHM={} should produce valid result",
                fwhm
            );
            let pos = result.unwrap();
            assert!(
                (pos.x - 64.0).abs() < 1.0,
                "FWHM={}: cx={} should be reasonable",
                fwhm,
                pos.x
            );
            assert!(
                (pos.y - 64.0).abs() < 1.0,
                "FWHM={}: cy={} should be reasonable",
                fwhm,
                pos.y
            );
        }
    }
}
