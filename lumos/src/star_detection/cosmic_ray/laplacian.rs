//! Laplacian-based cosmic ray detection.
//!
//! Implements the L.A.Cosmic algorithm edge detection using discrete Laplacian.
//! Cosmic rays have very sharp edges compared to astronomical sources (smoothed by PSF),
//! producing high Laplacian values.
//!
//! Reference: van Dokkum 2001, PASP 113, 1420

use super::simd;
use crate::common::Buffer2;
use glam::Vec2;

/// Compute the Laplacian of an image using a 3x3 kernel.
///
/// Uses the standard discrete Laplacian kernel:
/// ```text
///  0  1  0
///  1 -4  1
///  0  1  0
/// ```
///
/// Edge pixels are handled by clamping to image bounds.
/// Uses SIMD acceleration for interior pixels on supported platforms.
pub fn compute_laplacian(pixels: &Buffer2<f32>) -> Buffer2<f32> {
    let width = pixels.width();
    let height = pixels.height();

    // Use SIMD-accelerated version for larger images
    if width >= 10 && height >= 3 {
        simd::compute_laplacian_simd(pixels)
    } else {
        // Scalar fallback for small images
        let data = compute_laplacian_scalar(pixels, width, height);
        Buffer2::new(width, height, data)
    }
}

/// Scalar implementation of Laplacian computation.
fn compute_laplacian_scalar(pixels: &[f32], width: usize, height: usize) -> Vec<f32> {
    let mut laplacian = vec![0.0f32; pixels.len()];

    for y in 0..height {
        for x in 0..width {
            let idx = y * width + x;

            // Get neighbors with clamping
            let left = if x > 0 {
                pixels[y * width + (x - 1)]
            } else {
                pixels[idx]
            };
            let right = if x + 1 < width {
                pixels[y * width + (x + 1)]
            } else {
                pixels[idx]
            };
            let up = if y > 0 {
                pixels[(y - 1) * width + x]
            } else {
                pixels[idx]
            };
            let down = if y + 1 < height {
                pixels[(y + 1) * width + x]
            } else {
                pixels[idx]
            };

            // Laplacian = sum of neighbors - 4 * center
            laplacian[idx] = left + right + up + down - 4.0 * pixels[idx];
        }
    }

    laplacian
}

/// Compute L.A.Cosmic-style Laplacian SNR for a single star candidate.
///
/// This can be used as an additional metric in star detection to reject
/// cosmic rays. High values (>5) indicate cosmic ray-like sharp edges.
///
/// # Arguments
/// * `pixels` - Image pixel data
/// * `pos` - Star center position
/// * `stamp_radius` - Radius of analysis stamp
/// * `background` - Background level at star position
/// * `noise` - Noise level at star position
///
/// # Returns
/// Laplacian SNR value. Higher = more cosmic ray-like.
pub fn compute_laplacian_snr(
    pixels: &Buffer2<f32>,
    pos: Vec2,
    stamp_radius: usize,
    background: f32,
    noise: f32,
) -> f32 {
    let width = pixels.width();
    let height = pixels.height();
    let icx = pos.x.round() as isize;
    let icy = pos.y.round() as isize;

    // Check bounds
    let r = stamp_radius as isize;
    if icx < r || icy < r || icx + r >= width as isize || icy + r >= height as isize {
        return 0.0;
    }

    // Compute Laplacian at center (peak of star)
    let idx = icy as usize * width + icx as usize;
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
    fn test_compute_laplacian_flat_image() {
        // Flat image should have zero Laplacian
        let pixels = Buffer2::new(5, 5, vec![0.5f32; 25]);
        let laplacian = compute_laplacian(&pixels);

        // All interior points should be zero
        for y in 1..4 {
            for x in 1..4 {
                let idx = y * 5 + x;
                assert!(
                    laplacian[idx].abs() < 1e-6,
                    "Laplacian at ({},{}) = {}",
                    x,
                    y,
                    laplacian[idx]
                );
            }
        }
    }

    #[test]
    fn test_compute_laplacian_single_peak() {
        // Single pixel peak should have negative Laplacian (peak detection)
        let mut pixels_data = vec![0.0f32; 25];
        pixels_data[2 * 5 + 2] = 1.0; // Center peak
        let pixels = Buffer2::new(5, 5, pixels_data);

        let laplacian = compute_laplacian(&pixels);

        // Center should have negative Laplacian (sharp peak)
        assert!(
            laplacian[2 * 5 + 2] < -3.0,
            "Center Laplacian = {}",
            laplacian[2 * 5 + 2]
        );

        // Neighbors should have positive Laplacian
        assert!(laplacian[11] > 0.0); // 2*5+1
        assert!(laplacian[13] > 0.0); // 2*5+3
        assert!(laplacian[7] > 0.0); // 1*5+2
        assert!(laplacian[17] > 0.0); // 3*5+2
    }

    #[test]
    fn test_compute_laplacian_gaussian_peak() {
        // Gaussian peak should have smaller (less negative) Laplacian than sharp peak
        let mut sharp_data = vec![0.0f32; 49];
        sharp_data[3 * 7 + 3] = 1.0;
        let sharp = Buffer2::new(7, 7, sharp_data);

        let mut gaussian_data = vec![0.0f32; 49];
        // Simple 3x3 Gaussian-ish pattern
        gaussian_data[3 * 7 + 3] = 1.0;
        gaussian_data[3 * 7 + 2] = 0.5;
        gaussian_data[3 * 7 + 4] = 0.5;
        gaussian_data[2 * 7 + 3] = 0.5;
        gaussian_data[4 * 7 + 3] = 0.5;
        gaussian_data[2 * 7 + 2] = 0.25;
        gaussian_data[2 * 7 + 4] = 0.25;
        gaussian_data[4 * 7 + 2] = 0.25;
        gaussian_data[4 * 7 + 4] = 0.25;
        let gaussian = Buffer2::new(7, 7, gaussian_data);

        let lapl_sharp = compute_laplacian(&sharp);
        let lapl_gaussian = compute_laplacian(&gaussian);

        // Sharp peak has more negative Laplacian than Gaussian
        assert!(
            lapl_sharp[3 * 7 + 3] < lapl_gaussian[3 * 7 + 3],
            "Sharp: {}, Gaussian: {}",
            lapl_sharp[3 * 7 + 3],
            lapl_gaussian[3 * 7 + 3]
        );
    }

    #[test]
    fn test_compute_laplacian_snr_cosmic_ray() {
        // Sharp peak has high Laplacian SNR
        let mut pixels_data = vec![0.1f32; 49];
        pixels_data[3 * 7 + 3] = 1.0;
        let pixels = Buffer2::new(7, 7, pixels_data);

        let snr = compute_laplacian_snr(&pixels, Vec2::new(3.0, 3.0), 2, 0.1, 0.01);

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

        let gaussian_snr = compute_laplacian_snr(&pixels, Vec2::splat(center as f32), 3, 0.1, 0.01);

        // Compare to a cosmic ray (single sharp pixel)
        let mut cr_pixels_data = vec![0.1f32; size * size];
        cr_pixels_data[center * size + center] = 0.9;
        let cr_pixels = Buffer2::new(size, size, cr_pixels_data);

        let cr_snr = compute_laplacian_snr(&cr_pixels, Vec2::splat(center as f32), 3, 0.1, 0.01);

        // Cosmic ray should have significantly higher Laplacian SNR than Gaussian star
        assert!(
            cr_snr > gaussian_snr * 2.0,
            "Cosmic ray ({}) should have much higher Laplacian SNR than Gaussian star ({})",
            cr_snr,
            gaussian_snr
        );
    }
}
