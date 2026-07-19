//! Transform utilities for synthetic star fields.
//!
//! Provides functions to apply geometric transforms (translation, rotation, scale)
//! to `Star` positions for testing registration algorithms, plus helpers to
//! synthesize random fields.

use std::f64::consts::FRAC_PI_2;

use crate::{stacking::star_detection::star::Star, testing::TestRng};
use glam::DVec2;

/// Generate random star positions within a bounded area (default 50-px margin).
///
/// Uses a deterministic LCG random number generator for reproducibility.
pub fn generate_random_positions(
    num_stars: usize,
    width: f64,
    height: f64,
    seed: u64,
) -> Vec<DVec2> {
    let margin = 50.0;
    let mut rng = TestRng::new(seed);
    let mut stars = Vec::with_capacity(num_stars);

    for _ in 0..num_stars {
        let x = margin + rng.next_f64() * (width - 2.0 * margin);
        let y = margin + rng.next_f64() * (height - 2.0 * margin);
        stars.push(DVec2::new(x, y));
    }

    stars
}

/// Convert positions to Star structs with uniform properties.
///
/// Creates Star structs suitable for registration testing with default properties.
/// The FWHM is set uniformly to allow `register()` to derive `max_sigma` correctly.
pub fn positions_to_stars(positions: &[DVec2], fwhm: f32) -> Vec<Star> {
    positions
        .iter()
        .enumerate()
        .map(|(i, &pos)| Star {
            pos,
            flux: 10000.0 - i as f32 * 10.0,
            fwhm,
            eccentricity: 0.0,
            snr: 100.0,
            peak: 1.0,
            sharpness: 0.5,
            roundness1: 0.0,
            roundness2: 0.0,
        })
        .collect()
}

/// Generate random star positions and convert to Star structs.
pub fn generate_random_stars(
    num_stars: usize,
    width: f64,
    height: f64,
    seed: u64,
    fwhm: f32,
) -> Vec<Star> {
    let positions = generate_random_positions(num_stars, width, height, seed);
    positions_to_stars(&positions, fwhm)
}

/// Apply a translation transform to Stars.
pub fn translate_star_list(stars: &[Star], dx: f64, dy: f64) -> Vec<Star> {
    let offset = DVec2::new(dx, dy);
    stars
        .iter()
        .map(|s| Star {
            pos: s.pos + offset,
            ..*s
        })
        .collect()
}

/// Apply a similarity transform (translation + rotation + scale) to Stars.
///
/// The transform is applied around a center point: translate to origin relative to
/// center, rotate + scale, translate back, then apply the translation offset.
pub fn transform_star_list(
    stars: &[Star],
    dx: f64,
    dy: f64,
    angle_rad: f64,
    scale: f64,
    center_x: f64,
    center_y: f64,
) -> Vec<Star> {
    let cos_a = angle_rad.cos() * scale;
    let sin_a = angle_rad.sin() * scale;
    let center = DVec2::new(center_x, center_y);
    let offset = DVec2::new(dx, dy);

    stars
        .iter()
        .map(|s| {
            let r = s.pos - center;
            let new_x = cos_a * r.x - sin_a * r.y + center.x + offset.x;
            let new_y = sin_a * r.x + cos_a * r.y + center.y + offset.y;
            Star {
                pos: DVec2::new(new_x, new_y),
                ..*s
            }
        })
        .collect()
}

/// Add positional noise to Stars.
pub fn add_star_noise(stars: &[Star], noise_amplitude: f64, seed: u64) -> Vec<Star> {
    let mut rng = TestRng::new(seed);
    stars
        .iter()
        .map(|s| {
            let noise = DVec2::new(
                (rng.next_f64() * 2.0 - 1.0) * noise_amplitude,
                (rng.next_f64() * 2.0 - 1.0) * noise_amplitude,
            );
            Star {
                pos: s.pos + noise,
                ..*s
            }
        })
        .collect()
}

/// Remove random stars from a list (simulate missed detections).
pub fn remove_random_star_list(stars: &[Star], fraction: f64, seed: u64) -> Vec<Star> {
    assert!(
        (0.0..=1.0).contains(&fraction),
        "fraction must be between 0.0 and 1.0"
    );
    let mut rng = TestRng::new(seed);
    stars
        .iter()
        .filter(|_| rng.next_f64() >= fraction)
        .cloned()
        .collect()
}

/// Add random spurious stars (simulate false detections).
pub fn add_spurious_star_list(
    stars: &[Star],
    count: usize,
    width: f64,
    height: f64,
    seed: u64,
    fwhm: f32,
) -> Vec<Star> {
    let margin = 10.0;
    let mut result = stars.to_vec();
    let mut rng = TestRng::new(seed);

    for i in 0..count {
        let x = margin + rng.next_f64() * (width - 2.0 * margin);
        let y = margin + rng.next_f64() * (height - 2.0 * margin);
        result.push(Star {
            pos: DVec2::new(x, y),
            flux: 100.0 - i as f32,
            fwhm,
            eccentricity: 0.0,
            snr: 10.0,
            peak: 0.1,
            sharpness: 0.5,
            roundness1: 0.0,
            roundness2: 0.0,
        });
    }

    result
}

/// Filter Stars to a bounding box (simulate partial overlap).
pub fn filter_stars_to_bounds(
    stars: &[Star],
    min_x: f64,
    max_x: f64,
    min_y: f64,
    max_y: f64,
) -> Vec<Star> {
    stars
        .iter()
        .filter(|s| s.pos.x >= min_x && s.pos.x <= max_x && s.pos.y >= min_y && s.pos.y <= max_y)
        .cloned()
        .collect()
}

/// Translate Stars and keep only those that remain within the margin (partial overlap).
pub fn translate_stars_with_overlap(
    stars: &[Star],
    dx: f64,
    dy: f64,
    width: f64,
    height: f64,
    margin: f64,
) -> Vec<Star> {
    let offset = DVec2::new(dx, dy);
    stars
        .iter()
        .map(|s| Star {
            pos: s.pos + offset,
            ..*s
        })
        .filter(|s| {
            s.pos.x >= margin
                && s.pos.x <= width - margin
                && s.pos.y >= margin
                && s.pos.y <= height - margin
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::testing::synthetic::transforms::*;

    #[test]
    fn test_generate_random_positions() {
        let stars = generate_random_positions(100, 1000.0, 1000.0, 12345);
        assert_eq!(stars.len(), 100);

        // Check all stars are within bounds (with margin)
        for p in &stars {
            assert!(p.x >= 50.0 && p.x <= 950.0);
            assert!(p.y >= 50.0 && p.y <= 950.0);
        }

        // Check reproducibility
        let stars2 = generate_random_positions(100, 1000.0, 1000.0, 12345);
        assert_eq!(stars, stars2);
    }

    #[test]
    fn test_translate_star_list() {
        let stars = positions_to_stars(&[DVec2::new(100.0, 200.0), DVec2::new(300.0, 400.0)], 3.0);
        let translated = translate_star_list(&stars, 10.0, -20.0);

        assert_eq!(translated[0].pos, DVec2::new(110.0, 180.0));
        assert_eq!(translated[1].pos, DVec2::new(310.0, 380.0));
        // Non-position fields are preserved.
        assert_eq!(translated[0].flux, stars[0].flux);
    }

    #[test]
    fn test_transform_identity() {
        let stars = positions_to_stars(&[DVec2::new(100.0, 200.0)], 3.0);
        let transformed = transform_star_list(&stars, 0.0, 0.0, 0.0, 1.0, 500.0, 500.0);

        assert!((transformed[0].pos.x - 100.0).abs() < 1e-10);
        assert!((transformed[0].pos.y - 200.0).abs() < 1e-10);
    }

    #[test]
    fn test_transform_rotate_90_degrees() {
        // 100 px right of center → 100 px above center after a 90° rotation.
        let stars = positions_to_stars(&[DVec2::new(600.0, 500.0)], 3.0);
        let rotated = transform_star_list(&stars, 0.0, 0.0, FRAC_PI_2, 1.0, 500.0, 500.0);

        assert!((rotated[0].pos.x - 500.0).abs() < 1e-10);
        assert!((rotated[0].pos.y - 600.0).abs() < 1e-10);
    }

    #[test]
    fn test_transform_scale() {
        // 100 px right of center → 200 px right of center at 2× scale.
        let stars = positions_to_stars(&[DVec2::new(600.0, 500.0)], 3.0);
        let scaled = transform_star_list(&stars, 0.0, 0.0, 0.0, 2.0, 500.0, 500.0);

        assert!((scaled[0].pos.x - 700.0).abs() < 1e-10);
        assert!((scaled[0].pos.y - 500.0).abs() < 1e-10);
    }

    #[test]
    fn test_add_star_noise() {
        let stars = positions_to_stars(&vec![DVec2::new(500.0, 500.0); 100], 3.0);
        let noisy = add_star_noise(&stars, 1.0, 12345);

        let mut has_different = false;
        for (orig, noisy) in stars.iter().zip(noisy.iter()) {
            if (orig.pos.x - noisy.pos.x).abs() > 1e-10 || (orig.pos.y - noisy.pos.y).abs() > 1e-10
            {
                has_different = true;
            }
            // Noise stays within amplitude.
            assert!((orig.pos.x - noisy.pos.x).abs() <= 1.0);
            assert!((orig.pos.y - noisy.pos.y).abs() <= 1.0);
        }
        assert!(has_different);
    }

    #[test]
    fn test_remove_random_star_list() {
        let stars = positions_to_stars(&vec![DVec2::splat(1.0); 100], 3.0);
        // Same seed → same survivors; ~30% removed.
        let a = remove_random_star_list(&stars, 0.3, 99);
        let b = remove_random_star_list(&stars, 0.3, 99);
        assert_eq!(a.len(), b.len());
        assert!(a.len() < 100 && a.len() > 50);
    }
}
