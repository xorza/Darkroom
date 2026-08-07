//! Simple test patterns for benchmarks and tests.
//!
//! Provides basic image patterns like gradients, uniform fills, and checkerboards
//! that are commonly needed in benchmarks and tests.

use imaginarium::Buffer2;

use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;
use crate::testing::TestRng;

/// Create a uniform image filled with a single value.
pub(crate) fn uniform(size: Size2us, value: f32) -> Buffer2<f32> {
    Buffer2::new_filled(size.width, size.height, value)
}

/// Create a horizontal gradient from left to right.
pub(super) fn horizontal_gradient(size: Size2us, left: f32, right: f32) -> Buffer2<f32> {
    let mut pixels = vec![0.0f32; size.pixel_count()];
    for y in 0..size.height {
        for x in 0..size.width {
            let t = if size.width > 1 {
                x as f32 / (size.width - 1) as f32
            } else {
                0.5
            };
            pixels[size.index_of(Vec2us::new(x, y))] = left + t * (right - left);
        }
    }
    Buffer2::new(size.width, size.height, pixels)
}

/// Create a diagonal gradient for interpolation testing.
///
/// Formula: `(x + y * 0.5) / (width + height)`
/// This creates a gradient that varies in both X and Y directions,
/// making it useful for testing interpolation accuracy.
pub(crate) fn diagonal_gradient(size: Size2us) -> Buffer2<f32> {
    let scale = (size.width + size.height) as f32;
    let pixels: Vec<f32> = (0..size.height)
        .flat_map(|y| (0..size.width).map(move |x| (x as f32 + y as f32 * 0.5) / scale))
        .collect();
    Buffer2::new(size.width, size.height, pixels)
}

/// Create a checkerboard pattern.
///
/// Useful for phase correlation and registration tests.
pub(super) fn checkerboard(
    size: Size2us,
    cell_size: usize,
    value_a: f32,
    value_b: f32,
) -> Buffer2<f32> {
    let mut pixels = vec![0.0f32; size.pixel_count()];
    for y in 0..size.height {
        for x in 0..size.width {
            let checker = ((x / cell_size) + (y / cell_size)) % 2;
            pixels[size.index_of(Vec2us::new(x, y))] = if checker == 0 { value_a } else { value_b };
        }
    }
    Buffer2::new(size.width, size.height, pixels)
}

/// Add deterministic Gaussian noise to a pixel slice.
///
/// Uses Box-Muller transform via `TestRng::next_gaussian_f32()`.
/// This is the canonical noise helper — all test code should use this
/// instead of reimplementing Gaussian noise locally.
pub(crate) fn add_gaussian_noise(pixels: &mut [f32], sigma: f32, seed: u64) {
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
        let img = uniform(Size2us::new(10, 10), 0.5);
        assert_eq!(img.width(), 10);
        assert_eq!(img.height(), 10);
        for &p in img.iter() {
            assert!((p - 0.5).abs() < 1e-6);
        }
    }

    #[test]
    fn test_horizontal_gradient() {
        let img = horizontal_gradient(Size2us::new(100, 10), 0.0, 1.0);
        assert!((img[(0, 0)] - 0.0).abs() < 1e-6);
        assert!((img[(99, 0)] - 1.0).abs() < 1e-6);
        // Middle should be ~0.5
        assert!((img[(50, 5)] - 0.505).abs() < 0.01);
    }

    #[test]
    fn test_checkerboard() {
        let img = checkerboard(Size2us::new(16, 16), 4, 0.0, 1.0);
        assert!((img[(0, 0)] - 0.0).abs() < 1e-6);
        assert!((img[(4, 0)] - 1.0).abs() < 1e-6);
        assert!((img[(0, 4)] - 1.0).abs() < 1e-6);
        assert!((img[(4, 4)] - 0.0).abs() < 1e-6);
    }
}
