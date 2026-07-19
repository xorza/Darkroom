//! Simple test patterns for benchmarks and tests.
//!
//! Provides basic image patterns like gradients, uniform fills, and checkerboards
//! that are commonly needed in benchmarks and tests.

use imaginarium::Buffer2;

use crate::testing::TestRng;

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

/// Add deterministic Gaussian noise to a pixel slice.
///
/// Uses Box-Muller transform via `TestRng::next_gaussian_f32()`.
/// This is the canonical noise helper — all test code should use this
/// instead of reimplementing Gaussian noise locally.
pub fn add_gaussian_noise(pixels: &mut [f32], sigma: f32, seed: u64) {
    let mut rng = TestRng::new(seed);
    for p in pixels.iter_mut() {
        *p += rng.next_gaussian_f32() * sigma;
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::synthetic::patterns::*;

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
    fn test_checkerboard() {
        let img = checkerboard(16, 16, 4, 0.0, 1.0);
        assert!((img[(0, 0)] - 0.0).abs() < 1e-6);
        assert!((img[(4, 0)] - 1.0).abs() < 1e-6);
        assert!((img[(0, 4)] - 1.0).abs() < 1e-6);
        assert!((img[(4, 4)] - 0.0).abs() < 1e-6);
    }
}
