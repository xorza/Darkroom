//! Synthetic star field generation for testing star detection algorithms.

use crate::math::FWHM_TO_SIGMA;
use glam::Vec2;

/// A synthetic star to be placed in a generated image.
#[derive(Debug, Clone, Copy)]
pub struct SyntheticStar {
    /// Position (center).
    pub pos: Vec2,
    /// Peak brightness (0.0-1.0).
    pub brightness: f32,
    /// Sigma of the Gaussian profile (FWHM ≈ 2.355 * sigma).
    pub sigma: f32,
}

impl SyntheticStar {
    /// Create a new synthetic star.
    pub fn new(x: f32, y: f32, brightness: f32, sigma: f32) -> Self {
        Self {
            pos: Vec2::new(x, y),
            brightness,
            sigma,
        }
    }

    /// Get the FWHM (Full Width at Half Maximum) of this star.
    pub fn fwhm(&self) -> f32 {
        FWHM_TO_SIGMA * self.sigma
    }
}

/// Configuration for synthetic star field generation.
#[derive(Debug, Clone)]
pub struct SyntheticFieldConfig {
    /// Image width in pixels.
    pub width: usize,
    /// Image height in pixels.
    pub height: usize,
    /// Background level (0.0-1.0).
    pub background: f32,
    /// Background noise standard deviation.
    pub noise_sigma: f32,
}

impl Default for SyntheticFieldConfig {
    fn default() -> Self {
        Self {
            width: 256,
            height: 256,
            background: 0.1,
            noise_sigma: 0.01,
        }
    }
}

/// Generate a synthetic star field image.
///
/// Returns a grayscale image as a Vec<f32> with values in range 0.0-1.0.
pub fn generate_star_field(config: &SyntheticFieldConfig, stars: &[SyntheticStar]) -> Vec<f32> {
    let mut pixels = vec![config.background; config.width * config.height];

    // Add stars
    for star in stars {
        add_gaussian_star(&mut pixels, config.width, config.height, star);
    }

    // Add noise if configured
    if config.noise_sigma > 0.0 {
        crate::testing::synthetic::patterns::add_gaussian_noise(
            &mut pixels,
            config.noise_sigma,
            12345,
        );
    }

    // Clamp to valid range
    for p in &mut pixels {
        *p = p.clamp(0.0, 1.0);
    }

    pixels
}

/// Add a Gaussian star profile to the image.
fn add_gaussian_star(pixels: &mut [f32], width: usize, height: usize, star: &SyntheticStar) {
    // Only render within 4 sigma of center (covers >99.99% of flux)
    let radius = (4.0 * star.sigma).ceil() as i32;

    let cx = star.pos.x.round() as i32;
    let cy = star.pos.y.round() as i32;

    let x_min = (cx - radius).max(0) as usize;
    let x_max = ((cx + radius) as usize).min(width - 1);
    let y_min = (cy - radius).max(0) as usize;
    let y_max = ((cy + radius) as usize).min(height - 1);

    let two_sigma_sq = 2.0 * star.sigma * star.sigma;

    for y in y_min..=y_max {
        for x in x_min..=x_max {
            let dx = x as f32 - star.pos.x;
            let dy = y as f32 - star.pos.y;
            let r_sq = dx * dx + dy * dy;

            let value = star.brightness * (-r_sq / two_sigma_sq).exp();
            pixels[y * width + x] += value;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::GrayImage;

    /// Save synthetic star field to PNG for visual verification.
    #[test]
    fn test_save_synthetic_image() {
        let config = SyntheticFieldConfig {
            width: 256,
            height: 256,
            background: 0.05,
            noise_sigma: 0.01,
        };

        let stars = vec![
            SyntheticStar::new(64.0, 64.0, 0.9, 3.0),
            SyntheticStar::new(192.0, 64.0, 0.7, 2.5),
            SyntheticStar::new(64.0, 192.0, 0.5, 2.0),
            SyntheticStar::new(192.0, 192.0, 0.8, 4.0),
            SyntheticStar::new(128.0, 128.0, 0.6, 2.0),
        ];

        let pixels = generate_star_field(&config, &stars);

        // Convert to u8 grayscale
        let bytes: Vec<u8> = pixels
            .iter()
            .map(|&p| (p.clamp(0.0, 1.0) * 255.0) as u8)
            .collect();

        let image = GrayImage::from_raw(config.width as u32, config.height as u32, bytes).unwrap();

        let output_path =
            common::test_utils::test_output_path("synthetic_starfield/synthetic_stars.png");
        image.save(&output_path).unwrap();
        println!("Saved synthetic star field to: {:?}", output_path);
    }

    #[test]
    fn test_synthetic_star_fwhm() {
        let star = SyntheticStar::new(50.0, 50.0, 0.8, 2.0);
        let expected_fwhm = FWHM_TO_SIGMA * 2.0;
        assert!((star.fwhm() - expected_fwhm).abs() < 0.001);
    }

    #[test]
    fn test_generate_empty_field() {
        let config = SyntheticFieldConfig {
            width: 64,
            height: 64,
            background: 0.1,
            noise_sigma: 0.0,
        };

        let pixels = generate_star_field(&config, &[]);

        // All pixels should be background level
        for &p in &pixels {
            assert!((p - 0.1).abs() < 0.001);
        }
    }

    #[test]
    fn test_generate_single_star() {
        let config = SyntheticFieldConfig {
            width: 64,
            height: 64,
            background: 0.1,
            noise_sigma: 0.0,
        };

        let stars = vec![SyntheticStar::new(32.0, 32.0, 0.8, 2.0)];
        let pixels = generate_star_field(&config, &stars);

        // Peak should be at center
        let peak_idx = 32 * 64 + 32;
        let peak = pixels[peak_idx];

        // Peak should be background + brightness
        assert!(
            (peak - 0.9).abs() < 0.01,
            "Peak value {} not close to expected 0.9",
            peak
        );

        // Corner should be near background
        let corner = pixels[0];
        assert!(
            (corner - 0.1).abs() < 0.01,
            "Corner value {} not close to background 0.1",
            corner
        );
    }

    #[test]
    fn test_generate_with_noise() {
        let config = SyntheticFieldConfig {
            width: 64,
            height: 64,
            background: 0.5,
            noise_sigma: 0.05,
        };

        let pixels = generate_star_field(&config, &[]);

        // Calculate mean and std dev
        let mean: f32 = pixels.iter().sum::<f32>() / pixels.len() as f32;
        let variance: f32 =
            pixels.iter().map(|&p| (p - mean).powi(2)).sum::<f32>() / pixels.len() as f32;
        let std_dev = variance.sqrt();

        // Mean should be close to background
        assert!(
            (mean - 0.5).abs() < 0.02,
            "Mean {} not close to background 0.5",
            mean
        );

        // Std dev should be close to noise_sigma
        assert!(
            (std_dev - 0.05).abs() < 0.02,
            "Std dev {} not close to noise_sigma 0.05",
            std_dev
        );
    }

    #[test]
    fn test_star_at_subpixel_position() {
        let config = SyntheticFieldConfig {
            width: 64,
            height: 64,
            background: 0.0,
            noise_sigma: 0.0,
        };

        // Star at sub-pixel position
        let stars = vec![SyntheticStar::new(32.3, 32.7, 1.0, 2.0)];
        let pixels = generate_star_field(&config, &stars);

        // Find the actual peak pixel
        let (max_idx, _) = pixels
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())
            .unwrap();

        let max_x = max_idx % 64;
        let max_y = max_idx / 64;

        // Peak should be within 1 pixel of the star center
        assert!(
            (max_x as f32 - 32.3).abs() <= 1.0,
            "Peak X {} too far from star center 32.3",
            max_x
        );
        assert!(
            (max_y as f32 - 32.7).abs() <= 1.0,
            "Peak Y {} too far from star center 32.7",
            max_y
        );
    }
}
