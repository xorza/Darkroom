//! Small star stamp generation for centroid and profile fitting tests.
//!
//! Provides functions to generate small image stamps containing Gaussian star fields.

use common::Buffer2;

use crate::testing::TestRng;

/// Generate a star field stamp with multiple Gaussian stars.
///
/// Useful for benchmarking centroid computation on multiple stars.
///
/// # Arguments
/// * `width` - Stamp width in pixels
/// * `height` - Stamp height in pixels
/// * `num_stars` - Number of stars to generate
/// * `sigma` - Gaussian sigma for all stars
/// * `background` - Background level
/// * `seed` - Random seed for reproducibility
pub fn star_field(
    width: usize,
    height: usize,
    num_stars: usize,
    sigma: f32,
    background: f32,
    seed: u64,
) -> (Buffer2<f32>, Vec<(f32, f32)>) {
    let mut pixels = vec![background; width * height];
    let mut positions = Vec::with_capacity(num_stars);
    let mut rng = TestRng::new(seed);

    let margin = (sigma * 4.0).ceil() as usize;
    let two_sigma_sq = 2.0 * sigma * sigma;

    for _ in 0..num_stars {
        // Random position with margin from edges
        let cx =
            margin as f32 + rng.next_f32() * (width - 2 * margin) as f32 + rng.next_f32() * 0.5;
        let cy =
            margin as f32 + rng.next_f32() * (height - 2 * margin) as f32 + rng.next_f32() * 0.5;
        let brightness = 0.5 + rng.next_f32() * 0.5;

        // Add Gaussian star
        let radius = (sigma * 4.0).ceil() as i32;
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                let x = (cx as i32 + dx) as usize;
                let y = (cy as i32 + dy) as usize;
                if x < width && y < height {
                    let fx = x as f32 - cx;
                    let fy = y as f32 - cy;
                    let r2 = fx * fx + fy * fy;
                    let value = brightness * (-r2 / two_sigma_sq).exp();
                    pixels[y * width + x] += value;
                }
            }
        }
        positions.push((cx, cy));
    }

    (Buffer2::new(width, height, pixels), positions)
}

/// Generate a large star field for benchmarking detection algorithms.
///
/// This is optimized for benchmark use - generates stars with varying FWHM
/// and deterministic positions, plus adds background noise.
///
/// # Arguments
/// * `width` - Image width
/// * `height` - Image height
/// * `num_stars` - Number of stars to generate
/// * `background` - Background level
/// * `noise_amplitude` - Amplitude of deterministic noise
/// * `seed` - Random seed for reproducibility
pub fn benchmark_star_field(
    width: usize,
    height: usize,
    num_stars: usize,
    background: f32,
    noise_amplitude: f32,
    seed: u64,
) -> Buffer2<f32> {
    let mut pixels = vec![background; width * height];

    // Add deterministic noise using hash
    for (i, p) in pixels.iter_mut().enumerate() {
        let hash =
            ((i as u64).wrapping_mul(2654435761).wrapping_add(seed)) as f32 / (u64::MAX as f32);
        *p += (hash - 0.5) * 2.0 * noise_amplitude;
    }

    // Add synthetic stars
    let mut rng = TestRng::new(seed);

    let margin = 15;
    for _ in 0..num_stars {
        let cx = margin + ((rng.next_u64() >> 33) as usize % (width - 2 * margin));
        let cy = margin + ((rng.next_u64() >> 33) as usize % (height - 2 * margin));
        let brightness = 0.5 + rng.next_f32() * 0.5;
        let sigma = 1.5 + rng.next_f32();
        let two_sigma_sq = 2.0 * sigma * sigma;

        // Render star with limited radius for efficiency
        let radius = (sigma * 4.0).ceil() as i32;
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                let x = (cx as i32 + dx) as usize;
                let y = (cy as i32 + dy) as usize;
                if x < width && y < height {
                    let r2 = (dx * dx + dy * dy) as f32;
                    let value = brightness * (-r2 / two_sigma_sq).exp();
                    pixels[y * width + x] += value;
                }
            }
        }
    }

    Buffer2::new(width, height, pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_star_field() {
        let (pixels, positions) = star_field(256, 256, 10, 2.0, 0.1, 42);
        assert_eq!(positions.len(), 10);
        // All positions should be within bounds with margin
        for (x, y) in &positions {
            assert!(*x > 8.0 && *x < 248.0);
            assert!(*y > 8.0 && *y < 248.0);
        }
        // Pixels at star positions should be brighter than background
        for (x, y) in &positions {
            let px = *x as usize;
            let py = *y as usize;
            assert!(pixels[(px, py)] > 0.2);
        }
    }
}
