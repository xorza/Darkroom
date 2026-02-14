//! Transform utilities for synthetic star fields.
//!
//! Provides functions to apply geometric transforms (translation, rotation, scale)
//! to star positions for testing registration algorithms.
//!
//! Most functions are generic over the `Positioned` trait, supporting both
//! `DVec2` (bare positions) and `Star` (full star structs) transparently.

use crate::star_detection::Star;
use glam::DVec2;

// ============================================================================
// Positioned trait — abstracts over DVec2 and Star
// ============================================================================

/// Trait for types that have a 2D position and can be reconstructed with a new position.
pub trait Positioned: Clone {
    fn pos(&self) -> DVec2;
    fn with_pos(&self, pos: DVec2) -> Self;
}

impl Positioned for DVec2 {
    #[inline]
    fn pos(&self) -> DVec2 {
        *self
    }
    #[inline]
    fn with_pos(&self, pos: DVec2) -> Self {
        pos
    }
}

impl Positioned for Star {
    #[inline]
    fn pos(&self) -> DVec2 {
        self.pos
    }
    #[inline]
    fn with_pos(&self, pos: DVec2) -> Self {
        Star { pos, ..*self }
    }
}

// ============================================================================
// Generic implementations
// ============================================================================

fn translate_impl<T: Positioned>(items: &[T], dx: f64, dy: f64) -> Vec<T> {
    let offset = DVec2::new(dx, dy);
    items.iter().map(|p| p.with_pos(p.pos() + offset)).collect()
}

fn transform_impl<T: Positioned>(
    items: &[T],
    dx: f64,
    dy: f64,
    angle_rad: f64,
    scale: f64,
    center_x: f64,
    center_y: f64,
) -> Vec<T> {
    let cos_a = angle_rad.cos() * scale;
    let sin_a = angle_rad.sin() * scale;
    let center = DVec2::new(center_x, center_y);
    let offset = DVec2::new(dx, dy);

    items
        .iter()
        .map(|p| {
            let r = p.pos() - center;
            let new_x = cos_a * r.x - sin_a * r.y + center.x + offset.x;
            let new_y = sin_a * r.x + cos_a * r.y + center.y + offset.y;
            p.with_pos(DVec2::new(new_x, new_y))
        })
        .collect()
}

fn add_noise_impl<T: Positioned>(items: &[T], noise_amplitude: f64, seed: u64) -> Vec<T> {
    let mut rng = crate::testing::TestRng::new(seed);

    items
        .iter()
        .map(|p| {
            let noise = DVec2::new(
                (rng.next_f64() * 2.0 - 1.0) * noise_amplitude,
                (rng.next_f64() * 2.0 - 1.0) * noise_amplitude,
            );
            p.with_pos(p.pos() + noise)
        })
        .collect()
}

fn remove_random_impl<T: Positioned>(items: &[T], fraction: f64, seed: u64) -> Vec<T> {
    assert!(
        (0.0..=1.0).contains(&fraction),
        "fraction must be between 0.0 and 1.0"
    );

    let mut rng = crate::testing::TestRng::new(seed);

    items
        .iter()
        .filter(|_| rng.next_f64() >= fraction)
        .cloned()
        .collect()
}

fn filter_to_bounds_impl<T: Positioned>(
    items: &[T],
    min_x: f64,
    max_x: f64,
    min_y: f64,
    max_y: f64,
) -> Vec<T> {
    items
        .iter()
        .filter(|p| {
            let pos = p.pos();
            pos.x >= min_x && pos.x <= max_x && pos.y >= min_y && pos.y <= max_y
        })
        .cloned()
        .collect()
}

fn translate_with_overlap_impl<T: Positioned>(
    items: &[T],
    dx: f64,
    dy: f64,
    width: f64,
    height: f64,
    margin: f64,
) -> Vec<T> {
    let offset = DVec2::new(dx, dy);
    items
        .iter()
        .map(|p| p.with_pos(p.pos() + offset))
        .filter(|p| {
            let pos = p.pos();
            pos.x >= margin
                && pos.x <= width - margin
                && pos.y >= margin
                && pos.y <= height - margin
        })
        .collect()
}

// ============================================================================
// DVec2 public API
// ============================================================================

/// Generate random star positions within a bounded area.
///
/// Uses a deterministic LCG random number generator for reproducibility.
pub fn generate_random_positions(
    num_stars: usize,
    width: f64,
    height: f64,
    seed: u64,
) -> Vec<DVec2> {
    generate_random_positions_with_margin(num_stars, width, height, seed, 50.0)
}

/// Generate random star positions with custom margin.
pub fn generate_random_positions_with_margin(
    num_stars: usize,
    width: f64,
    height: f64,
    seed: u64,
    margin: f64,
) -> Vec<DVec2> {
    let mut rng = crate::testing::TestRng::new(seed);
    let mut stars = Vec::with_capacity(num_stars);

    for _ in 0..num_stars {
        let x = margin + rng.next_f64() * (width - 2.0 * margin);
        let y = margin + rng.next_f64() * (height - 2.0 * margin);
        stars.push(DVec2::new(x, y));
    }

    stars
}

/// Apply a translation transform to a star field.
pub fn translate_stars(stars: &[DVec2], dx: f64, dy: f64) -> Vec<DVec2> {
    translate_impl(stars, dx, dy)
}

/// Apply a similarity transform (translation + rotation + scale) to a star field.
///
/// The transform is applied around a center point:
/// 1. Translate star to origin (relative to center)
/// 2. Apply rotation and scale
/// 3. Translate back to center
/// 4. Apply translation offset
pub fn transform_stars(
    stars: &[DVec2],
    dx: f64,
    dy: f64,
    angle_rad: f64,
    scale: f64,
    center_x: f64,
    center_y: f64,
) -> Vec<DVec2> {
    transform_impl(stars, dx, dy, angle_rad, scale, center_x, center_y)
}

/// Apply rotation only (no translation or scale) around a center point.
pub fn rotate_stars(stars: &[DVec2], angle_rad: f64, center_x: f64, center_y: f64) -> Vec<DVec2> {
    transform_impl(stars, 0.0, 0.0, angle_rad, 1.0, center_x, center_y)
}

/// Apply scale only (no translation or rotation) around a center point.
pub fn scale_stars(stars: &[DVec2], scale: f64, center_x: f64, center_y: f64) -> Vec<DVec2> {
    transform_impl(stars, 0.0, 0.0, 0.0, scale, center_x, center_y)
}

/// Add positional noise to star coordinates.
pub fn add_position_noise(stars: &[DVec2], noise_amplitude: f64, seed: u64) -> Vec<DVec2> {
    add_noise_impl(stars, noise_amplitude, seed)
}

/// Remove random stars from a list (simulate missed detections).
pub fn remove_random_stars(stars: &[DVec2], fraction: f64, seed: u64) -> Vec<DVec2> {
    remove_random_impl(stars, fraction, seed)
}

/// Add random spurious stars (simulate false detections).
pub fn add_spurious_stars(
    stars: &[DVec2],
    count: usize,
    width: f64,
    height: f64,
    seed: u64,
) -> Vec<DVec2> {
    let margin = 10.0;
    let mut result = stars.to_vec();
    let mut rng = crate::testing::TestRng::new(seed);

    for _ in 0..count {
        let x = margin + rng.next_f64() * (width - 2.0 * margin);
        let y = margin + rng.next_f64() * (height - 2.0 * margin);
        result.push(DVec2::new(x, y));
    }

    result
}

/// Filter stars to a bounding box (simulate partial overlap).
pub fn filter_to_bounds(
    stars: &[DVec2],
    min_x: f64,
    max_x: f64,
    min_y: f64,
    max_y: f64,
) -> Vec<DVec2> {
    filter_to_bounds_impl(stars, min_x, max_x, min_y, max_y)
}

/// Simulate partial overlap by shifting stars and keeping only those in bounds.
pub fn translate_with_overlap(
    stars: &[DVec2],
    dx: f64,
    dy: f64,
    width: f64,
    height: f64,
    margin: f64,
) -> Vec<DVec2> {
    translate_with_overlap_impl(stars, dx, dy, width, height, margin)
}

// ============================================================================
// Star public API (delegates to generic implementations)
// ============================================================================

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
    translate_impl(stars, dx, dy)
}

/// Apply a similarity transform to Stars.
pub fn transform_star_list(
    stars: &[Star],
    dx: f64,
    dy: f64,
    angle_rad: f64,
    scale: f64,
    center_x: f64,
    center_y: f64,
) -> Vec<Star> {
    transform_impl(stars, dx, dy, angle_rad, scale, center_x, center_y)
}

/// Add positional noise to Stars.
pub fn add_star_noise(stars: &[Star], noise_amplitude: f64, seed: u64) -> Vec<Star> {
    add_noise_impl(stars, noise_amplitude, seed)
}

/// Remove random stars from a list.
pub fn remove_random_star_list(stars: &[Star], fraction: f64, seed: u64) -> Vec<Star> {
    remove_random_impl(stars, fraction, seed)
}

/// Add random spurious stars.
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
    let mut rng = crate::testing::TestRng::new(seed);

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

/// Filter Stars to a bounding box.
pub fn filter_stars_to_bounds(
    stars: &[Star],
    min_x: f64,
    max_x: f64,
    min_y: f64,
    max_y: f64,
) -> Vec<Star> {
    filter_to_bounds_impl(stars, min_x, max_x, min_y, max_y)
}

/// Translate Stars with overlap filtering.
pub fn translate_stars_with_overlap(
    stars: &[Star],
    dx: f64,
    dy: f64,
    width: f64,
    height: f64,
    margin: f64,
) -> Vec<Star> {
    translate_with_overlap_impl(stars, dx, dy, width, height, margin)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_translate_stars() {
        let stars = vec![DVec2::new(100.0, 200.0), DVec2::new(300.0, 400.0)];
        let translated = translate_stars(&stars, 10.0, -20.0);

        assert_eq!(translated[0], DVec2::new(110.0, 180.0));
        assert_eq!(translated[1], DVec2::new(310.0, 380.0));
    }

    #[test]
    fn test_transform_stars_identity() {
        let stars = vec![DVec2::new(100.0, 200.0), DVec2::new(300.0, 400.0)];
        let transformed = transform_stars(&stars, 0.0, 0.0, 0.0, 1.0, 500.0, 500.0);

        // Identity transform should preserve positions
        assert!((transformed[0].x - 100.0).abs() < 1e-10);
        assert!((transformed[0].y - 200.0).abs() < 1e-10);
    }

    #[test]
    fn test_rotate_stars_90_degrees() {
        let stars = vec![DVec2::new(600.0, 500.0)]; // 100 pixels right of center
        let rotated = rotate_stars(&stars, std::f64::consts::FRAC_PI_2, 500.0, 500.0);

        // 90 degree rotation should move point to 100 pixels above center
        assert!((rotated[0].x - 500.0).abs() < 1e-10);
        assert!((rotated[0].y - 600.0).abs() < 1e-10);
    }

    #[test]
    fn test_scale_stars() {
        let stars = vec![DVec2::new(600.0, 500.0)]; // 100 pixels right of center
        let scaled = scale_stars(&stars, 2.0, 500.0, 500.0);

        // 2x scale should move point to 200 pixels right of center
        assert!((scaled[0].x - 700.0).abs() < 1e-10);
        assert!((scaled[0].y - 500.0).abs() < 1e-10);
    }

    #[test]
    fn test_add_position_noise() {
        let stars = vec![DVec2::new(500.0, 500.0); 100];
        let noisy = add_position_noise(&stars, 1.0, 12345);

        // Check that noise was added
        let mut has_different = false;
        for (orig, noisy) in stars.iter().zip(noisy.iter()) {
            if (orig.x - noisy.x).abs() > 1e-10 || (orig.y - noisy.y).abs() > 1e-10 {
                has_different = true;
            }
            // Noise should be within amplitude
            assert!((orig.x - noisy.x).abs() <= 1.0);
            assert!((orig.y - noisy.y).abs() <= 1.0);
        }
        assert!(has_different);
    }

    #[test]
    fn test_positioned_trait_dvec2_and_star_consistent() {
        // Verify that DVec2 and Star versions produce identical positions
        let positions = vec![DVec2::new(100.0, 200.0), DVec2::new(300.0, 400.0)];
        let stars = positions_to_stars(&positions, 3.0);

        // translate
        let dvec_result = translate_stars(&positions, 10.0, -5.0);
        let star_result = translate_star_list(&stars, 10.0, -5.0);
        for (d, s) in dvec_result.iter().zip(star_result.iter()) {
            assert_eq!(*d, s.pos);
        }

        // transform
        let dvec_result = transform_stars(&positions, 5.0, 3.0, 0.1, 1.05, 200.0, 300.0);
        let star_result = transform_star_list(&stars, 5.0, 3.0, 0.1, 1.05, 200.0, 300.0);
        for (d, s) in dvec_result.iter().zip(star_result.iter()) {
            assert!((d.x - s.pos.x).abs() < 1e-10);
            assert!((d.y - s.pos.y).abs() < 1e-10);
        }

        // noise (same seed → same positions)
        let dvec_noisy = add_position_noise(&positions, 0.5, 42);
        let star_noisy = add_star_noise(&stars, 0.5, 42);
        for (d, s) in dvec_noisy.iter().zip(star_noisy.iter()) {
            assert!((d.x - s.pos.x).abs() < 1e-10);
            assert!((d.y - s.pos.y).abs() < 1e-10);
        }

        // remove_random (same seed → same survivors)
        let dvec_filtered = remove_random_stars(&positions, 0.3, 99);
        let star_filtered = remove_random_star_list(&stars, 0.3, 99);
        assert_eq!(dvec_filtered.len(), star_filtered.len());
        for (d, s) in dvec_filtered.iter().zip(star_filtered.iter()) {
            assert_eq!(*d, s.pos);
        }
    }
}
