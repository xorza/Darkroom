//! Artifact generators for synthetic test images.
//!
//! Provides various artifacts commonly found in astronomical images:
//! - Cosmic rays
//! - Hot/dead pixels
//! - Bad columns/rows
//! - Satellite/airplane trails
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

    let mut rng = seed;
    let next_u64 = |s: &mut u64| -> u64 {
        *s = s.wrapping_mul(6364136223846793005).wrapping_add(1);
        *s
    };
    let next_f32 = |s: &mut u64| -> f32 { (next_u64(s) >> 33) as f32 / (1u64 << 31) as f32 };

    for _ in 0..count {
        let x = (next_f32(&mut rng) * width as f32) as usize;
        let y = (next_f32(&mut rng) * height as f32) as usize;
        let amp = amplitude_range.0 + next_f32(&mut rng) * (amplitude_range.1 - amplitude_range.0);

        if x < width && y < height {
            // Single pixel hit (most cosmic rays)
            pixels[y * width + x] += amp;

            // Some cosmic rays have slight bleeding
            if next_f32(&mut rng) > 0.7 {
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

/// Add hot pixels (constant high values) to the image.
pub fn add_hot_pixels(
    pixels: &mut [f32],
    width: usize,
    positions: &[(usize, usize)],
    amplitude: f32,
) {
    let height = pixels.len() / width;
    for &(x, y) in positions {
        if x < width && y < height {
            pixels[y * width + x] += amplitude;
        }
    }
}

/// Add dead pixels (zero response) to the image.
pub fn add_dead_pixels(pixels: &mut [f32], width: usize, positions: &[(usize, usize)]) {
    let height = pixels.len() / width;
    for &(x, y) in positions {
        if x < width && y < height {
            pixels[y * width + x] = 0.0;
        }
    }
}

/// Add bad columns to the image.
///
/// Bad columns can be either hot (elevated values) or dead (zero).
pub fn add_bad_columns(
    pixels: &mut [f32],
    width: usize,
    columns: &[usize],
    mode: BadPixelMode,
    value: f32,
) {
    let height = pixels.len() / width;
    for &col in columns {
        if col < width {
            for y in 0..height {
                match mode {
                    BadPixelMode::Hot => pixels[y * width + col] += value,
                    BadPixelMode::Dead => pixels[y * width + col] = 0.0,
                    BadPixelMode::Fixed => pixels[y * width + col] = value,
                }
            }
        }
    }
}

/// Add bad rows to the image.
pub fn add_bad_rows(
    pixels: &mut [f32],
    width: usize,
    rows: &[usize],
    mode: BadPixelMode,
    value: f32,
) {
    let height = pixels.len() / width;
    for &row in rows {
        if row < height {
            for x in 0..width {
                match mode {
                    BadPixelMode::Hot => pixels[row * width + x] += value,
                    BadPixelMode::Dead => pixels[row * width + x] = 0.0,
                    BadPixelMode::Fixed => pixels[row * width + x] = value,
                }
            }
        }
    }
}

/// Mode for bad pixel behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BadPixelMode {
    /// Add value to existing pixel
    Hot,
    /// Set pixel to zero
    Dead,
    /// Set pixel to fixed value
    Fixed,
}

/// Add a linear trail (satellite or airplane).
///
/// # Arguments
/// * `pixels` - Mutable pixel buffer
/// * `width` - Image width
/// * `start` - Trail start position (x, y)
/// * `end` - Trail end position (x, y)
/// * `trail_width` - Width of the trail in pixels
/// * `amplitude` - Peak brightness of the trail
pub fn add_linear_trail(
    pixels: &mut [f32],
    width: usize,
    start: (f32, f32),
    end: (f32, f32),
    trail_width: f32,
    amplitude: f32,
) {
    let height = pixels.len() / width;

    let dx = end.0 - start.0;
    let dy = end.1 - start.1;
    let length = (dx * dx + dy * dy).sqrt();

    if length < 0.001 {
        return;
    }

    // Unit vector along trail
    let ux = dx / length;
    let uy = dy / length;

    // Perpendicular vector
    let px = -uy;
    let py = ux;

    let half_width = trail_width / 2.0;

    for y in 0..height {
        for x in 0..width {
            // Vector from start to this pixel
            let vx = x as f32 - start.0;
            let vy = y as f32 - start.1;

            // Project onto trail direction
            let along = vx * ux + vy * uy;

            // Skip if outside trail length
            if along < 0.0 || along > length {
                continue;
            }

            // Distance perpendicular to trail
            let perp = (vx * px + vy * py).abs();

            // Apply Gaussian profile across width
            if perp < half_width * 3.0 {
                let profile = (-perp * perp / (2.0 * half_width * half_width / 4.0)).exp();
                pixels[y * width + x] += amplitude * profile;
            }
        }
    }
}

/// Add CFA (Bayer) pattern artifacts.
///
/// This simulates the checkerboard pattern visible in debayered images
/// when color channels have different sensitivities.
///
/// # Arguments
/// * `pixels` - Mutable pixel buffer
/// * `width` - Image width
/// * `strength` - Amplitude of the pattern (0.0-1.0)
/// * `pattern` - Bayer pattern type
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

/// Generate random hot pixel positions.
pub fn generate_random_hot_pixels(
    width: usize,
    height: usize,
    count: usize,
    seed: u64,
) -> Vec<(usize, usize)> {
    let mut positions = Vec::with_capacity(count);

    let mut rng = seed;
    let next_f32 = |s: &mut u64| -> f32 {
        *s = s.wrapping_mul(6364136223846793005).wrapping_add(1);
        (*s >> 33) as f32 / (1u64 << 31) as f32
    };

    for _ in 0..count {
        let x = (next_f32(&mut rng) * width as f32) as usize;
        let y = (next_f32(&mut rng) * height as f32) as usize;
        positions.push((x.min(width - 1), y.min(height - 1)));
    }

    positions
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

    #[test]
    fn test_bad_column() {
        let width = 64;
        let height = 64;
        let mut pixels = vec![0.1f32; width * height];

        add_bad_columns(&mut pixels, width, &[10], BadPixelMode::Dead, 0.0);

        // Column 10 should be zero
        for y in 0..height {
            assert_eq!(pixels[y * width + 10], 0.0);
        }

        // Other columns should be unchanged
        assert_eq!(pixels[0], 0.1);
    }

    #[test]
    fn test_linear_trail() {
        let width = 64;
        let height = 64;
        let mut pixels = vec![0.0f32; width * height];

        add_linear_trail(&mut pixels, width, (0.0, 32.0), (63.0, 32.0), 3.0, 0.5);

        // Center of trail should have elevated values
        assert!(pixels[32 * width + 32] > 0.3);

        // Far from trail should be zero
        assert!(pixels[0] < 0.01);
    }
}
