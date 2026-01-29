//! Cosmic ray detection using L.A.Cosmic algorithm.
//!
//! Implementation based on van Dokkum 2001, PASP 113, 1420:
//! "Cosmic-Ray Rejection by Laplacian Edge Detection"
//!
//! The key insight is that cosmic rays have very sharp edges compared to
//! astronomical sources, which are smoothed by the PSF. The Laplacian
//! (second derivative) responds strongly to sharp edges.
//!
//! Algorithm overview:
//! 1. Compute Laplacian of image (detects sharp edges)
//! 2. Compare Laplacian to noise-normalized expectation
//! 3. Pixels with high Laplacian/noise ratio are cosmic rays
//!
//! This approach is more robust than simple peak/flux ratio (sharpness)
//! for extended cosmic ray tracks and clusters.

mod fine_structure;
mod laplacian;

#[cfg(feature = "bench")]
pub mod bench;

// SIMD implementations (not yet implemented)
pub mod simd;

#[cfg(test)]
mod tests;

use crate::common::Buffer2;

// Re-export public API
pub use fine_structure::compute_fine_structure;
// median_of_n is now in median_filter module
pub use laplacian::{compute_laplacian, compute_laplacian_snr};

/// Configuration for L.A.Cosmic cosmic ray detection.
#[derive(Debug, Clone)]
pub struct LACosmicConfig {
    /// Sigma threshold for cosmic ray detection.
    /// Pixels with Laplacian > threshold * noise are flagged.
    /// Typical value: 4.0-5.0
    pub sigma_clip: f32,
    /// Object detection limit (sigma above sky).
    /// Pixels below this are considered background.
    /// Typical value: 4.0-5.0
    pub obj_lim: f32,
    /// Fine structure growth radius in pixels.
    /// Controls how much to grow detected regions.
    /// Typical value: 1
    pub grow_radius: usize,
}

impl Default for LACosmicConfig {
    fn default() -> Self {
        Self {
            sigma_clip: 4.5,
            obj_lim: 5.0,
            grow_radius: 1,
        }
    }
}

/// Result of L.A.Cosmic cosmic ray detection.
#[derive(Debug)]
pub struct LACosmicResult {
    /// Boolean mask where true = cosmic ray pixel.
    #[allow(dead_code)]
    pub cosmic_ray_mask: Buffer2<bool>,
    /// Number of cosmic ray pixels detected.
    #[allow(dead_code)]
    pub cosmic_ray_count: usize,
    /// Laplacian image (for debugging).
    #[allow(dead_code)]
    pub laplacian: Buffer2<f32>,
}

/// Detect cosmic rays using the L.A.Cosmic algorithm.
///
/// # Arguments
/// * `pixels` - Image pixel data (grayscale, normalized 0.0-1.0)
/// * `background` - Background level per pixel
/// * `noise` - Noise level per pixel (sigma)
/// * `config` - Detection configuration
///
/// # Returns
/// LACosmicResult containing the cosmic ray mask and diagnostics.
#[allow(dead_code)]
pub fn detect_cosmic_rays(
    pixels: &Buffer2<f32>,
    background: &Buffer2<f32>,
    noise: &Buffer2<f32>,
    config: &LACosmicConfig,
) -> LACosmicResult {
    let width = pixels.width();
    let height = pixels.height();
    debug_assert_eq!(width, background.width());
    debug_assert_eq!(height, background.height());
    debug_assert_eq!(width, noise.width());
    debug_assert_eq!(height, noise.height());

    // Step 1: Compute Laplacian of background-subtracted image
    // First subtract background
    let subtracted_data: Vec<f32> = pixels
        .iter()
        .zip(background.iter())
        .map(|(&p, &b)| (p - b).max(0.0))
        .collect();
    let subtracted = Buffer2::new(width, height, subtracted_data);

    // Compute Laplacian
    let laplacian_img = compute_laplacian(&subtracted);

    // Step 2: Compute fine structure image for comparison
    let fine_structure = compute_fine_structure(&subtracted);

    // Step 3: Create initial cosmic ray mask
    // A pixel is flagged if:
    // 1. Laplacian is significantly negative (indicating sharp peak)
    // 2. The pixel is above object detection threshold
    // 3. Laplacian magnitude exceeds fine structure (stars spread Laplacian, CRs don't)
    let mut mask = vec![false; pixels.len()];
    let mut cosmic_ray_count = 0;

    for i in 0..pixels.len() {
        let sigma = noise[i].max(1e-10);

        // Laplacian is negative at peaks (second derivative)
        // We want the magnitude
        let lapl_magnitude = (-laplacian_img[i]).max(0.0);

        // Normalize by noise
        let lapl_snr = lapl_magnitude / sigma;

        // Check if pixel is above object detection threshold
        let obj_above_bg = subtracted[i] / sigma;

        // Fine structure normalized by noise (kept for potential future use)
        let _fine_snr = fine_structure[i] / sigma;

        // Cosmic ray criteria based on L.A.Cosmic (van Dokkum 2001):
        // 1. High Laplacian SNR (sharp edges)
        // 2. Pixel is above detection threshold (not just noise)
        // 3. The Laplacian-to-flux ratio is very high (sharper than expected for PSF)
        //
        // For a Gaussian PSF with sigma, the Laplacian at center is:
        //   L = -2 * peak / sigma^2
        // For sigma=2: L/peak = -0.5
        // For sigma=1: L/peak = -2.0
        // For single pixel (sigma~0.5): L/peak = -4.0 (all flux in one pixel, none in neighbors)
        //
        // We normalize by the pixel value to get a "sharpness" measure independent of brightness
        let pixel_value = subtracted[i];
        let lapl_to_flux = if pixel_value > f32::EPSILON {
            lapl_magnitude / pixel_value
        } else {
            0.0
        };

        // Cosmic rays have lapl_to_flux > 3.0 (very sharp)
        // Well-sampled stars (sigma >= 2) have lapl_to_flux < 1.0
        if lapl_snr > config.sigma_clip && obj_above_bg > config.obj_lim && lapl_to_flux > 3.0 {
            mask[i] = true;
            cosmic_ray_count += 1;
        }
    }

    // Step 4: Grow the mask to catch cosmic ray wings
    if config.grow_radius > 0 {
        let grown_mask = grow_mask(&mask, width, height, config.grow_radius, &subtracted);
        let new_count = grown_mask.iter().filter(|&&m| m).count();
        cosmic_ray_count = new_count;
        mask = grown_mask;
    }

    LACosmicResult {
        cosmic_ray_mask: Buffer2::new(width, height, mask),
        cosmic_ray_count,
        laplacian: laplacian_img,
    }
}

/// Grow cosmic ray mask to neighboring pixels that are also elevated.
fn grow_mask(
    mask: &[bool],
    width: usize,
    height: usize,
    radius: usize,
    pixels: &Buffer2<f32>,
) -> Vec<bool> {
    let mut grown = mask.to_vec();

    for y in 0..height {
        for x in 0..width {
            let idx = y * width + x;
            if !mask[idx] {
                continue;
            }

            // Check neighbors within radius
            let y_start = y.saturating_sub(radius);
            let y_end = (y + radius + 1).min(height);
            let x_start = x.saturating_sub(radius);
            let x_end = (x + radius + 1).min(width);

            for ny in y_start..y_end {
                for nx in x_start..x_end {
                    let nidx = ny * width + nx;
                    // Only grow to pixels that are elevated (above median background)
                    if pixels[nidx] > 0.01 {
                        grown[nidx] = true;
                    }
                }
            }
        }
    }

    grown
}
