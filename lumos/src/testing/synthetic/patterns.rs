//! Simple test patterns for benchmarks and tests.
//!
//! Provides basic image patterns like gradients, uniform fills, and checkerboards
//! that are commonly needed in benchmarks and tests.

use crate::common::Buffer2;

/// Create a uniform image filled with a single value.
pub fn uniform(width: usize, height: usize, value: f32) -> Buffer2<f32> {
    Buffer2::new_filled(width, height, value)
}

/// Create a horizontal gradient from left to right.
pub fn horizontal_gradient(width: usize, height: usize, left: f32, right: f32) -> Buffer2<f32> {
    let mut pixels = vec![0.0f32; width * height];
    for y in 0..height {
        for x in 0..width {
            let t = if width > 1 {
                x as f32 / (width - 1) as f32
            } else {
                0.5
            };
            pixels[y * width + x] = left + t * (right - left);
        }
    }
    Buffer2::new(width, height, pixels)
}

/// Create a vertical gradient from top to bottom.
pub fn vertical_gradient(width: usize, height: usize, top: f32, bottom: f32) -> Buffer2<f32> {
    let mut pixels = vec![0.0f32; width * height];
    for y in 0..height {
        let t = if height > 1 {
            y as f32 / (height - 1) as f32
        } else {
            0.5
        };
        let value = top + t * (bottom - top);
        for x in 0..width {
            pixels[y * width + x] = value;
        }
    }
    Buffer2::new(width, height, pixels)
}

/// Create a diagonal gradient for interpolation testing.
///
/// Formula: `(x + y * 0.5) / (width + height)`
/// This creates a gradient that varies in both X and Y directions,
/// making it useful for testing interpolation accuracy.
pub fn diagonal_gradient(width: usize, height: usize) -> Buffer2<f32> {
    let scale = (width + height) as f32;
    let pixels: Vec<f32> = (0..height)
        .flat_map(|y| (0..width).map(move |x| (x as f32 + y as f32 * 0.5) / scale))
        .collect();
    Buffer2::new(width, height, pixels)
}

/// Create a radial gradient from center to edges.
pub fn radial_gradient(width: usize, height: usize, center: f32, edge: f32) -> Buffer2<f32> {
    use glam::Vec2;

    let mut pixels = vec![0.0f32; width * height];
    let c = Vec2::new(width as f32 / 2.0, height as f32 / 2.0);
    let max_r = c.length();

    for y in 0..height {
        for x in 0..width {
            let pixel_pos = Vec2::new(x as f32, y as f32);
            let r = pixel_pos.distance(c);
            let t = if max_r > 0.0 { r / max_r } else { 0.0 };
            pixels[y * width + x] = center + t * (edge - center);
        }
    }
    Buffer2::new(width, height, pixels)
}

/// Create a checkerboard pattern.
///
/// Useful for phase correlation and registration tests.
pub fn checkerboard(
    width: usize,
    height: usize,
    cell_size: usize,
    value_a: f32,
    value_b: f32,
) -> Buffer2<f32> {
    let mut pixels = vec![0.0f32; width * height];
    for y in 0..height {
        for x in 0..width {
            let checker = ((x / cell_size) + (y / cell_size)) % 2;
            pixels[y * width + x] = if checker == 0 { value_a } else { value_b };
        }
    }
    Buffer2::new(width, height, pixels)
}

/// Create a checkerboard pattern with optional offset (for shifted pattern tests).
pub fn checkerboard_offset(
    width: usize,
    height: usize,
    cell_size: usize,
    value_a: f32,
    value_b: f32,
    offset_x: isize,
    offset_y: isize,
) -> Buffer2<f32> {
    let mut pixels = vec![0.0f32; width * height];
    for y in 0..height {
        for x in 0..width {
            let xx = (x as isize - offset_x).rem_euclid(width as isize) as usize;
            let yy = (y as isize - offset_y).rem_euclid(height as isize) as usize;
            let checker = ((xx / cell_size) + (yy / cell_size)) % 2;
            pixels[y * width + x] = if checker == 0 { value_a } else { value_b };
        }
    }
    Buffer2::new(width, height, pixels)
}

/// Add deterministic uniform noise to an image buffer.
///
/// Uses a simple LCG-based hash for reproducible noise.
/// Each pixel gets a random offset in `[-amplitude, +amplitude]`.
pub fn add_noise(pixels: &mut Buffer2<f32>, amplitude: f32, seed: u64) {
    let mut rng = crate::testing::TestRng::new(seed);
    for p in pixels.iter_mut() {
        let hash = rng.next_f32();
        *p += (hash - 0.5) * 2.0 * amplitude;
    }
}

/// Add deterministic Gaussian noise to a pixel slice.
///
/// Uses Box-Muller transform via `TestRng::next_gaussian_f32()`.
/// This is the canonical noise helper — all test code should use this
/// instead of reimplementing Gaussian noise locally.
pub fn add_gaussian_noise(pixels: &mut [f32], sigma: f32, seed: u64) {
    let mut rng = crate::testing::TestRng::new(seed);
    for p in pixels.iter_mut() {
        *p += rng.next_gaussian_f32() * sigma;
    }
}

/// Create an image with deterministic noise.
pub fn noise(width: usize, height: usize, mean: f32, amplitude: f32, seed: u64) -> Buffer2<f32> {
    let mut pixels = Buffer2::new_filled(width, height, mean);
    add_noise(&mut pixels, amplitude, seed);
    pixels
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uniform() {
        let img = uniform(10, 10, 0.5);
        assert_eq!(img.width(), 10);
        assert_eq!(img.height(), 10);
        for &p in img.iter() {
            assert!((p - 0.5).abs() < 1e-6);
        }
    }

    #[test]
    fn test_horizontal_gradient() {
        let img = horizontal_gradient(100, 10, 0.0, 1.0);
        assert!((img[(0, 0)] - 0.0).abs() < 1e-6);
        assert!((img[(99, 0)] - 1.0).abs() < 1e-6);
        // Middle should be ~0.5
        assert!((img[(50, 5)] - 0.505).abs() < 0.01);
    }

    #[test]
    fn test_vertical_gradient() {
        let img = vertical_gradient(10, 100, 0.0, 1.0);
        assert!((img[(0, 0)] - 0.0).abs() < 1e-6);
        assert!((img[(0, 99)] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_radial_gradient() {
        let img = radial_gradient(100, 100, 1.0, 0.0);
        // Center should be close to 1.0
        assert!((img[(50, 50)] - 1.0).abs() < 0.1);
        // Corners should be closer to 0.0
        assert!(img[(0, 0)] < 0.3);
    }

    #[test]
    fn test_checkerboard() {
        let img = checkerboard(16, 16, 4, 0.0, 1.0);
        assert!((img[(0, 0)] - 0.0).abs() < 1e-6);
        assert!((img[(4, 0)] - 1.0).abs() < 1e-6);
        assert!((img[(0, 4)] - 1.0).abs() < 1e-6);
        assert!((img[(4, 4)] - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_noise_reproducibility() {
        let img1 = noise(100, 100, 0.5, 0.1, 42);
        let img2 = noise(100, 100, 0.5, 0.1, 42);
        for (p1, p2) in img1.iter().zip(img2.iter()) {
            assert!((p1 - p2).abs() < 1e-6);
        }
    }
}
