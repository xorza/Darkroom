//! Transform utilities for synthetic star fields.
//!
//! Provides functions to apply geometric transforms (translation, rotation, scale)
//! to star positions for testing registration algorithms.

use crate::star_detection::Star;
use glam::DVec2;

/// Generate random star positions within a bounded area.
///
/// Uses a deterministic LCG random number generator for reproducibility.
///
/// # Arguments
/// * `num_stars` - Number of star positions to generate
/// * `width` - Image width in pixels
/// * `height` - Image height in pixels
/// * `seed` - Random seed for reproducibility
/// * `margin` - Margin from edges (default 50.0 if not specified)
///
/// # Returns
/// Vector of DVec2 coordinates
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
    // Simple LCG random number generator for reproducibility
    let mut state = seed;
    let mut next_random = || {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        (state >> 33) as f64 / (1u64 << 31) as f64
    };

    let mut stars = Vec::with_capacity(num_stars);

    for _ in 0..num_stars {
        let x = margin + next_random() * (width - 2.0 * margin);
        let y = margin + next_random() * (height - 2.0 * margin);
        stars.push(DVec2::new(x, y));
    }

    stars
}

/// Apply a translation transform to a star field.
///
/// # Arguments
/// * `stars` - Input star positions
/// * `dx` - Translation in X direction (pixels)
/// * `dy` - Translation in Y direction (pixels)
///
/// # Returns
/// Translated star positions
pub fn translate_stars(stars: &[DVec2], dx: f64, dy: f64) -> Vec<DVec2> {
    let offset = DVec2::new(dx, dy);
    stars.iter().map(|p| *p + offset).collect()
}

/// Apply a similarity transform (translation + rotation + scale) to a star field.
///
/// The transform is applied around a center point:
/// 1. Translate star to origin (relative to center)
/// 2. Apply rotation and scale
/// 3. Translate back to center
/// 4. Apply translation offset
///
/// # Arguments
/// * `stars` - Input star positions
/// * `dx` - Translation in X direction (pixels)
/// * `dy` - Translation in Y direction (pixels)
/// * `angle_rad` - Rotation angle in radians
/// * `scale` - Scale factor (1.0 = no change)
/// * `center_x` - X coordinate of rotation center
/// * `center_y` - Y coordinate of rotation center
///
/// # Returns
/// Transformed star positions
pub fn transform_stars(
    stars: &[DVec2],
    dx: f64,
    dy: f64,
    angle_rad: f64,
    scale: f64,
    center_x: f64,
    center_y: f64,
) -> Vec<DVec2> {
    let cos_a = angle_rad.cos() * scale;
    let sin_a = angle_rad.sin() * scale;
    let center = DVec2::new(center_x, center_y);
    let offset = DVec2::new(dx, dy);

    stars
        .iter()
        .map(|p| {
            // Translate to origin, rotate+scale, translate back, then apply offset
            let r = *p - center;
            let new_x = cos_a * r.x - sin_a * r.y + center.x + offset.x;
            let new_y = sin_a * r.x + cos_a * r.y + center.y + offset.y;
            DVec2::new(new_x, new_y)
        })
        .collect()
}

/// Apply rotation only (no translation or scale) around a center point.
///
/// # Arguments
/// * `stars` - Input star positions
/// * `angle_rad` - Rotation angle in radians
/// * `center_x` - X coordinate of rotation center
/// * `center_y` - Y coordinate of rotation center
pub fn rotate_stars(stars: &[DVec2], angle_rad: f64, center_x: f64, center_y: f64) -> Vec<DVec2> {
    transform_stars(stars, 0.0, 0.0, angle_rad, 1.0, center_x, center_y)
}

/// Apply scale only (no translation or rotation) around a center point.
///
/// # Arguments
/// * `stars` - Input star positions
/// * `scale` - Scale factor
/// * `center_x` - X coordinate of scale center
/// * `center_y` - Y coordinate of scale center
pub fn scale_stars(stars: &[DVec2], scale: f64, center_x: f64, center_y: f64) -> Vec<DVec2> {
    transform_stars(stars, 0.0, 0.0, 0.0, scale, center_x, center_y)
}

/// Add positional noise to star coordinates.
///
/// Uses deterministic LCG for reproducibility.
///
/// # Arguments
/// * `stars` - Input star positions
/// * `noise_amplitude` - Maximum noise in each direction (pixels)
/// * `seed` - Random seed
pub fn add_position_noise(stars: &[DVec2], noise_amplitude: f64, seed: u64) -> Vec<DVec2> {
    let mut state = seed;
    let mut next_random = || {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        ((state >> 33) as f64 / (1u64 << 31) as f64) * 2.0 - 1.0 // -1 to 1
    };

    stars
        .iter()
        .map(|p| {
            let noise = DVec2::new(
                next_random() * noise_amplitude,
                next_random() * noise_amplitude,
            );
            *p + noise
        })
        .collect()
}

/// Remove random stars from a list (simulate missed detections).
///
/// # Arguments
/// * `stars` - Input star positions
/// * `fraction` - Fraction of stars to remove (0.0 to 1.0)
/// * `seed` - Random seed for reproducibility
///
/// # Returns
/// Filtered star list with approximately (1 - fraction) of original stars
pub fn remove_random_stars(stars: &[DVec2], fraction: f64, seed: u64) -> Vec<DVec2> {
    assert!(
        (0.0..=1.0).contains(&fraction),
        "fraction must be between 0.0 and 1.0"
    );

    let mut state = seed;
    let mut next_random = || {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        (state >> 33) as f64 / (1u64 << 31) as f64
    };

    stars
        .iter()
        .filter(|_| next_random() >= fraction)
        .copied()
        .collect()
}

/// Add random spurious stars (simulate false detections like hot pixels, cosmic rays).
///
/// # Arguments
/// * `stars` - Input star positions
/// * `count` - Number of spurious stars to add
/// * `width` - Image width for bounds
/// * `height` - Image height for bounds
/// * `seed` - Random seed for reproducibility
///
/// # Returns
/// Star list with additional spurious positions appended
pub fn add_spurious_stars(
    stars: &[DVec2],
    count: usize,
    width: f64,
    height: f64,
    seed: u64,
) -> Vec<DVec2> {
    let margin = 10.0;
    let mut result = stars.to_vec();

    let mut state = seed;
    let mut next_random = || {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        (state >> 33) as f64 / (1u64 << 31) as f64
    };

    for _ in 0..count {
        let x = margin + next_random() * (width - 2.0 * margin);
        let y = margin + next_random() * (height - 2.0 * margin);
        result.push(DVec2::new(x, y));
    }

    result
}

/// Filter stars to a bounding box (simulate partial overlap).
///
/// # Arguments
/// * `stars` - Input star positions
/// * `min_x` - Minimum X coordinate
/// * `max_x` - Maximum X coordinate
/// * `min_y` - Minimum Y coordinate
/// * `max_y` - Maximum Y coordinate
///
/// # Returns
/// Stars within the specified bounds
pub fn filter_to_bounds(
    stars: &[DVec2],
    min_x: f64,
    max_x: f64,
    min_y: f64,
    max_y: f64,
) -> Vec<DVec2> {
    stars
        .iter()
        .filter(|p| p.x >= min_x && p.x <= max_x && p.y >= min_y && p.y <= max_y)
        .copied()
        .collect()
}

/// Simulate partial overlap by shifting stars and keeping only those in bounds.
///
/// This simulates what happens when two images have partial overlap:
/// stars at edges are not visible in both images.
///
/// # Arguments
/// * `stars` - Input star positions
/// * `dx` - Translation in X
/// * `dy` - Translation in Y
/// * `width` - Image width
/// * `height` - Image height
/// * `margin` - Edge margin for valid positions
///
/// # Returns
/// Translated stars that remain within bounds
pub fn translate_with_overlap(
    stars: &[DVec2],
    dx: f64,
    dy: f64,
    width: f64,
    height: f64,
    margin: f64,
) -> Vec<DVec2> {
    let offset = DVec2::new(dx, dy);
    stars
        .iter()
        .map(|p| *p + offset)
        .filter(|p| {
            p.x >= margin && p.x <= width - margin && p.y >= margin && p.y <= height - margin
        })
        .collect()
}

/// Convert positions to Star structs with uniform properties.
///
/// Creates Star structs suitable for registration testing with default properties.
/// The FWHM is set uniformly to allow `register()` to derive `max_sigma` correctly.
///
/// # Arguments
/// * `positions` - Star positions as DVec2
/// * `fwhm` - FWHM value to use for all stars (controls max_sigma in registration)
///
/// # Returns
/// Vector of Star structs
pub fn positions_to_stars(positions: &[DVec2], fwhm: f32) -> Vec<Star> {
    positions
        .iter()
        .enumerate()
        .map(|(i, &pos)| Star {
            pos,
            flux: 10000.0 - i as f32 * 10.0, // Decreasing flux for sorting
            fwhm,
            eccentricity: 0.0,
            snr: 100.0,
            peak: 1.0,
            sharpness: 0.5,
            roundness1: 0.0,
            roundness2: 0.0,
            laplacian_snr: 50.0,
        })
        .collect()
}

/// Generate random star positions and convert to Star structs.
///
/// Convenience function combining `generate_random_positions` and `positions_to_stars`.
///
/// # Arguments
/// * `num_stars` - Number of stars to generate
/// * `width` - Image width in pixels
/// * `height` - Image height in pixels
/// * `seed` - Random seed for reproducibility
/// * `fwhm` - FWHM value to use for all stars
///
/// # Returns
/// Vector of Star structs with random positions
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
    let mut state = seed;
    let mut next_random = || {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        ((state >> 33) as f64 / (1u64 << 31) as f64) * 2.0 - 1.0
    };

    stars
        .iter()
        .map(|s| {
            let noise = DVec2::new(
                next_random() * noise_amplitude,
                next_random() * noise_amplitude,
            );
            Star {
                pos: s.pos + noise,
                ..*s
            }
        })
        .collect()
}

/// Remove random stars from a list.
pub fn remove_random_star_list(stars: &[Star], fraction: f64, seed: u64) -> Vec<Star> {
    assert!(
        (0.0..=1.0).contains(&fraction),
        "fraction must be between 0.0 and 1.0"
    );

    let mut state = seed;
    let mut next_random = || {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        (state >> 33) as f64 / (1u64 << 31) as f64
    };

    stars
        .iter()
        .filter(|_| next_random() >= fraction)
        .cloned()
        .collect()
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

    let mut state = seed;
    let mut next_random = || {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        (state >> 33) as f64 / (1u64 << 31) as f64
    };

    for i in 0..count {
        let x = margin + next_random() * (width - 2.0 * margin);
        let y = margin + next_random() * (height - 2.0 * margin);
        result.push(Star {
            pos: DVec2::new(x, y),
            flux: 100.0 - i as f32, // Low flux for spurious stars
            fwhm,
            eccentricity: 0.0,
            snr: 10.0,
            peak: 0.1,
            sharpness: 0.5,
            roundness1: 0.0,
            roundness2: 0.0,
            laplacian_snr: 5.0,
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
    stars
        .iter()
        .filter(|s| s.pos.x >= min_x && s.pos.x <= max_x && s.pos.y >= min_y && s.pos.y <= max_y)
        .cloned()
        .collect()
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
}
