//! Artifact generators for synthetic test images.
//!
//! Provides various artifacts commonly found in astronomical images:
//! - Cosmic rays
//! - CFA (Bayer) pattern artifacts

use std::ops::Range;

use common::Buffer2;

use crate::image::cfa::cfa_color::CfaColor;
use crate::image::cfa::cfa_pattern::CfaPattern;
use crate::internals::test_rng::TestRng;
use crate::math::vec2us::Vec2us;

/// `count` random cosmic-ray hits, each of an amplitude drawn uniformly from `amplitude`, placed
/// by the generator `seed` starts.
#[derive(Debug, Clone)]
pub(crate) struct CosmicRays {
    pub(crate) count: usize,
    pub(crate) amplitude: Range<f32>,
    pub(crate) seed: u64,
}

impl CosmicRays {
    /// Add the hits to `pixels`; where each landed, for verification.
    #[expect(
        clippy::cast_sign_loss,
        reason = "synthetic fixtures are small images with non-negative coordinates"
    )]
    pub(crate) fn add_to(self, pixels: &mut Buffer2<f32>) -> Vec<Vec2us> {
        let Self {
            count,
            amplitude,
            seed,
        } = self;
        let (width, height) = (pixels.width(), pixels.height());
        let mut poss = Vec::with_capacity(count);

        let mut rng = TestRng::new(seed);

        for _ in 0..count {
            let x = (rng.next_f32() * width as f32) as usize;
            let y = (rng.next_f32() * height as f32) as usize;
            let amp = amplitude.start + rng.next_f32() * (amplitude.end - amplitude.start);

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

            poss.push(Vec2us::new(x, y));
        }

        poss
    }
}

/// Add CFA (Bayer) pattern artifacts.
///
/// This simulates the checkerboard pattern visible in debayered images
/// when color channels have different sensitivities.
pub(crate) fn add_bayer_pattern(pixels: &mut Buffer2<f32>, strength: f32, pattern: CfaPattern) {
    let (width, height) = (pixels.width(), pixels.height());
    let r_factor = 1.0 + strength * 0.5;
    let b_factor = 1.0 - strength * 0.3;

    for y in 0..height {
        for x in 0..width {
            let factor = match pattern.color_at(Vec2us::new(x, y)) {
                CfaColor::Red => r_factor,
                CfaColor::Blue => b_factor,
                CfaColor::Green => 1.0,
            };

            let idx = y * width + x;
            pixels[idx] *= factor;
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::internals::synthetic::artifacts::*;

    #[test]
    fn cosmic_rays_count() {
        let mut pixels = Buffer2::new_default(64, 64);

        let poss = CosmicRays {
            count: 10,
            amplitude: 0.5..1.0,
            seed: 12345,
        }
        .add_to(&mut pixels);

        assert_eq!(poss.len(), 10);

        // Check that pixels were modified
        let non_zero_count = pixels.iter().filter(|&&p| p > 0.0).count();
        assert!(non_zero_count >= 10);
    }
}
