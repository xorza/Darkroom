//! Laplacian-based cosmic ray detection.
//!
//! Implements the L.A.Cosmic algorithm edge detection using discrete Laplacian.
//! Cosmic rays have very sharp edges compared to astronomical sources (smoothed by PSF),
//! producing high Laplacian values.
//!
//! Reference: van Dokkum 2001, PASP 113, 1420

use crate::common::Buffer2;
use crate::math::Vec2us;

/// Compute L.A.Cosmic-style Laplacian SNR for a single star candidate.
///
/// This can be used as an additional metric in star detection to reject
/// cosmic rays. High values (>5) indicate cosmic ray-like sharp edges.
///
/// # Arguments
/// * `pixels` - Image pixel data
/// * `pos` - Star center position (pixel coordinates)
/// * `stamp_radius` - Radius of analysis stamp
/// * `background` - Background level at star position
/// * `noise` - Noise level at star position
///
/// # Returns
/// Laplacian SNR value. Higher = more cosmic ray-like.
pub fn compute_laplacian_snr(
    pixels: &Buffer2<f32>,
    pos: Vec2us,
    stamp_radius: usize,
    background: f32,
    noise: f32,
) -> f32 {
    let width = pixels.width();
    let height = pixels.height();

    // Check bounds
    if pos.x < stamp_radius
        || pos.y < stamp_radius
        || pos.x + stamp_radius >= width
        || pos.y + stamp_radius >= height
    {
        return 0.0;
    }

    // Compute Laplacian at center (peak of star)
    let idx = pos.to_index(width);
    let center = pixels[idx] - background;

    let left = pixels[idx - 1] - background;
    let right = pixels[idx + 1] - background;
    let up = pixels[idx - width] - background;
    let down = pixels[idx + width] - background;

    // Laplacian at peak (negative for peaks)
    let laplacian = left + right + up + down - 4.0 * center;

    // Return normalized magnitude
    (-laplacian).max(0.0) / noise.max(1e-10)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_laplacian_snr_cosmic_ray() {
        // Sharp peak has high Laplacian SNR
        let mut pixels_data = vec![0.1f32; 49];
        pixels_data[3 * 7 + 3] = 1.0;
        let pixels = Buffer2::new(7, 7, pixels_data);

        let snr = compute_laplacian_snr(&pixels, Vec2us::new(3, 3), 2, 0.1, 0.01);

        assert!(
            snr > 50.0,
            "Cosmic ray should have high Laplacian SNR: {}",
            snr
        );
    }

    #[test]
    fn test_compute_laplacian_snr_gaussian_star() {
        // Gaussian star with real seeing-like sigma has lower Laplacian SNR than cosmic ray
        let size = 15;
        let center = 7;
        let sigma = 2.0f32; // Wider Gaussian simulating real seeing
        let mut pixels_data = vec![0.1f32; size * size];

        for y in 0..size {
            for x in 0..size {
                let dx = x as f32 - center as f32;
                let dy = y as f32 - center as f32;
                let r2 = dx * dx + dy * dy;
                let value = 0.8 * (-r2 / (2.0 * sigma * sigma)).exp();
                if value > 0.001 {
                    pixels_data[y * size + x] = 0.1 + value;
                }
            }
        }
        let pixels = Buffer2::new(size, size, pixels_data);

        let gaussian_snr =
            compute_laplacian_snr(&pixels, Vec2us::new(center, center), 3, 0.1, 0.01);

        // Compare to a cosmic ray (single sharp pixel)
        let mut cr_pixels_data = vec![0.1f32; size * size];
        cr_pixels_data[center * size + center] = 0.9;
        let cr_pixels = Buffer2::new(size, size, cr_pixels_data);

        let cr_snr = compute_laplacian_snr(&cr_pixels, Vec2us::new(center, center), 3, 0.1, 0.01);

        // Cosmic ray should have significantly higher Laplacian SNR than Gaussian star
        assert!(
            cr_snr > gaussian_snr * 2.0,
            "Cosmic ray ({}) should have much higher Laplacian SNR than Gaussian star ({})",
            cr_snr,
            gaussian_snr
        );
    }
}
