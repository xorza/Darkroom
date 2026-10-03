//! Simple test patterns for benchmarks and tests: gradients, noise, and a synthetic linear master.

use imaginarium::Buffer2;

use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::linear::LinearImage;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;
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

/// A horizontal gradient from `left` at the first column to `right` at the last.
pub(crate) fn horizontal_gradient(size: Size2us, left: f32, right: f32) -> Buffer2<f32> {
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

/// Add seeded Gaussian noise of a constant `sigma` to a pixel slice — for a fixture that wants one
/// fixed noise level. The sensor's own signal-dependent noise is [`render`]'s, through `noise`.
///
/// [`render`]: crate::testing::synthetic::observe::render
pub(crate) fn add_gaussian_noise(pixels: &mut [f32], sigma: f32, seed: u64) {
    let mut rng = TestRng::new(seed);
    for p in pixels.iter_mut() {
        *p += rng.next_gaussian_f32() * sigma;
    }
}

/// A synthetic linear RGB master: a sky gradient down the frame plus a hashed dither, channels
/// scaled 1, 0.9, 0.8, and every 9973rd pixel — a prime, so the cores do not align to a row — a
/// star core 1.5 above it, past 1.0 as a real stack's are. Representative enough that no image op
/// short-circuits on it.
pub(crate) fn linear_rgb_master(dimensions: ImageDimensions) -> LinearImage {
    let (width, height) = (dimensions.width(), dimensions.height());
    let count = width * height;
    let mut channels = [
        vec![0.0f32; count],
        vec![0.0f32; count],
        vec![0.0f32; count],
    ];
    for y in 0..height {
        for x in 0..width {
            let index = y * width + x;
            let sky = 0.02 + (y as f32 / height as f32) * 0.03;
            let hash = (index as u32).wrapping_mul(2_654_435_761) as f32 / u32::MAX as f32;
            let noise = (hash - 0.5) * 0.004;
            let core = if index % 9973 == 0 { 1.5 } else { 0.0 };
            for (channel, plane) in channels.iter_mut().enumerate() {
                plane[index] = sky * (1.0 - 0.1 * channel as f32) + noise + core;
            }
        }
    }
    LinearImage::from_planar_channels(dimensions, channels)
}
