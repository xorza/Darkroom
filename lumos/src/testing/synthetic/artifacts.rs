//! Artifact generators for synthetic test images.
//!
//! Provides various artifacts commonly found in astronomical images:
//! - Cosmic rays
//! - CFA (Bayer) pattern artifacts

use crate::io::raw::demosaic::bayer::CfaPattern;
use crate::math::vec2us::Vec2us;
use crate::testing::test_rng::TestRng;

/// Add random cosmic ray hits to the image.
///
/// Returns the positions of added cosmic rays for verification.
#[expect(
    clippy::cast_sign_loss,
    reason = "synthetic fixtures are small images with non-negative coordinates"
)]
pub(crate) fn add_cosmic_rays(
    pixels: &mut [f32],
    width: usize,
    count: usize,
    amplitude_range: (f32, f32),
    seed: u64,
) -> Vec<Vec2us> {
    let height = pixels.len() / width;
    let mut positions = Vec::with_capacity(count);

    let mut rng = TestRng::new(seed);

    for _ in 0..count {
        let x = (rng.next_f32() * width as f32) as usize;
        let y = (rng.next_f32() * height as f32) as usize;
        let amp = amplitude_range.0 + rng.next_f32() * (amplitude_range.1 - amplitude_range.0);

        // Single pixel hit (most cosmic rays)
        pixels[y * width + x] += amp;

        // Some cosmic rays have slight bleeding
        if rng.next_f32() > 0.7 {
            let bleed = amp * 0.15;
            if x > 0 {
                pixels[y * width + x - 1] += bleed;
            }
            if x < width - 1 {
                pixels[y * width + x + 1] += bleed;
            }
        }

        positions.push(Vec2us::new(x, y));
    }

    positions
}

/// Add CFA (Bayer) pattern artifacts.
///
/// This simulates the checkerboard pattern visible in debayered images
/// when color channels have different sensitivities.
pub(crate) fn add_bayer_pattern(
    pixels: &mut [f32],
    width: usize,
    strength: f32,
    pattern: CfaPattern,
) {
    let height = pixels.len() / width;

    // Apply slight variations to simulate different color channel gains
    let r_factor = 1.0 + strength * 0.5;
    let b_factor = 1.0 - strength * 0.3;

    for y in 0..height {
        for x in 0..width {
            let factor = match pattern.color_at(Vec2us::new(x, y)) {
                0 => r_factor,
                2 => b_factor,
                _ => 1.0,
            };

            let idx = y * width + x;
            pixels[idx] *= factor;
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::synthetic::artifacts::*;

    #[test]
    fn cosmic_rays_count() {
        let width = 64;
        let height = 64;
        let mut pixels = vec![0.0f32; width * height];

        let positions = add_cosmic_rays(&mut pixels, width, 10, (0.5, 1.0), 12345);

        assert_eq!(positions.len(), 10);

        // Check that pixels were modified
        let non_zero_count = pixels.iter().filter(|&&p| p > 0.0).count();
        assert!(non_zero_count >= 10);
    }
}
