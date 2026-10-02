//! Simple test patterns for benchmarks and tests.
//!
//! Provides the gradients benchmarks and tests need.

use imaginarium::Buffer2;

use crate::math::size2us::Size2us;
use crate::testing::test_rng::TestRng;

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
