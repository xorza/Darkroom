//! Artifact generators for synthetic test images.
//!
//! Provides various artifacts commonly found in astronomical images:
//! - Cosmic rays
//! - CFA (Bayer) pattern artifacts

/// Add random cosmic ray hits to the image.
///
/// Returns the positions of added cosmic rays for verification.
pub fn add_cosmic_rays(
    pixels: &mut [f32],
    width: usize,
    count: usize,
    amplitude_range: (f32, f32),
    seed: u64,
) -> Vec<(usize, usize)> {
    let height = pixels.len() / width;
    let mut positions = Vec::with_capacity(count);

    let mut rng = crate::testing::TestRng::new(seed);

    for _ in 0..count {
        let x = (rng.next_f32() * width as f32) as usize;
        let y = (rng.next_f32() * height as f32) as usize;
        let amp = amplitude_range.0 + rng.next_f32() * (amplitude_range.1 - amplitude_range.0);

        if x < width && y < height {
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

            positions.push((x, y));
        }
    }

    positions
}

/// Add CFA (Bayer) pattern artifacts.
///
/// This simulates the checkerboard pattern visible in debayered images
/// when color channels have different sensitivities.
pub fn add_bayer_pattern(pixels: &mut [f32], width: usize, strength: f32, pattern: BayerPattern) {
    let height = pixels.len() / width;

    // Pattern offsets for RGGB, GRBG, etc.
    let (r_offset, _g1_offset, _g2_offset, b_offset) = match pattern {
        BayerPattern::RGGB => ((0, 0), (1, 0), (0, 1), (1, 1)),
        BayerPattern::GRBG => ((1, 0), (0, 0), (1, 1), (0, 1)),
        BayerPattern::GBRG => ((0, 1), (0, 0), (1, 1), (1, 0)),
        BayerPattern::BGGR => ((1, 1), (1, 0), (0, 1), (0, 0)),
    };

    // Apply slight variations to simulate different color channel gains
    let r_factor = 1.0 + strength * 0.5;
    let b_factor = 1.0 - strength * 0.3;

    for y in 0..height {
        for x in 0..width {
            let phase_x = x % 2;
            let phase_y = y % 2;

            let factor = if (phase_x, phase_y) == r_offset {
                r_factor
            } else if (phase_x, phase_y) == b_offset {
                b_factor
            } else {
                1.0
            };

            let idx = y * width + x;
            pixels[idx] *= factor;
        }
    }
}

/// Bayer pattern types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::upper_case_acronyms)]
pub enum BayerPattern {
    RGGB,
    GRBG,
    GBRG,
    BGGR,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosmic_rays_count() {
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
