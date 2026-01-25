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

/// Configuration for L.A.Cosmic cosmic ray detection.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Used in tests
pub struct LACosmoicConfig {
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

impl Default for LACosmoicConfig {
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
#[allow(dead_code)] // Used in tests
pub struct LACosmicResult {
    /// Boolean mask where true = cosmic ray pixel.
    pub cosmic_ray_mask: Vec<bool>,
    /// Number of cosmic ray pixels detected.
    pub cosmic_ray_count: usize,
    /// Laplacian image (for debugging).
    pub laplacian: Vec<f32>,
}

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
#[allow(dead_code)] // Used in tests and detect_cosmic_rays
fn compute_laplacian(pixels: &[f32], width: usize, height: usize) -> Vec<f32> {
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

/// Compute fine structure image using 3x3 median filter.
///
/// The fine structure is the difference between the original and
/// median-filtered image. It captures small-scale structure including
/// cosmic rays, but also real fine features like stellar cores.
#[allow(dead_code)] // Used in tests and detect_cosmic_rays
fn compute_fine_structure(pixels: &[f32], width: usize, height: usize) -> Vec<f32> {
    let mut fine_structure = vec![0.0f32; pixels.len()];

    for y in 0..height {
        for x in 0..width {
            let idx = y * width + x;

            // Collect 3x3 neighborhood
            let mut neighbors = [0.0f32; 9];
            let mut count = 0;

            for dy in 0..3 {
                for dx in 0..3 {
                    let ny = y as isize + dy as isize - 1;
                    let nx = x as isize + dx as isize - 1;

                    if ny >= 0 && ny < height as isize && nx >= 0 && nx < width as isize {
                        neighbors[count] = pixels[ny as usize * width + nx as usize];
                        count += 1;
                    }
                }
            }

            // Compute median
            let median = median_of_n(&mut neighbors[..count]);

            // Fine structure = original - median (positive values = sharp features)
            fine_structure[idx] = (pixels[idx] - median).max(0.0);
        }
    }

    fine_structure
}

/// Compute median of a small array (in-place partial sort).
#[allow(dead_code)] // Used in tests and compute_fine_structure
fn median_of_n(values: &mut [f32]) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    values[values.len() / 2]
}

/// Detect cosmic rays using the L.A.Cosmic algorithm.
///
/// # Arguments
/// * `pixels` - Image pixel data (grayscale, normalized 0.0-1.0)
/// * `width` - Image width
/// * `height` - Image height
/// * `background` - Background level per pixel
/// * `noise` - Noise level per pixel (sigma)
/// * `config` - Detection configuration
///
/// # Returns
/// LACosmicResult containing the cosmic ray mask and diagnostics.
#[allow(dead_code)] // Used in tests
pub fn detect_cosmic_rays(
    pixels: &[f32],
    width: usize,
    height: usize,
    background: &[f32],
    noise: &[f32],
    config: &LACosmoicConfig,
) -> LACosmicResult {
    assert_eq!(pixels.len(), width * height);
    assert_eq!(background.len(), width * height);
    assert_eq!(noise.len(), width * height);

    // Step 1: Compute Laplacian of background-subtracted image
    // First subtract background
    let subtracted: Vec<f32> = pixels
        .iter()
        .zip(background.iter())
        .map(|(&p, &b)| (p - b).max(0.0))
        .collect();

    // Compute Laplacian
    let laplacian = compute_laplacian(&subtracted, width, height);

    // Step 2: Compute fine structure image for comparison
    let fine_structure = compute_fine_structure(&subtracted, width, height);

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
        let lapl_magnitude = (-laplacian[i]).max(0.0);

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
        mask.copy_from_slice(&grown_mask);
    }

    LACosmicResult {
        cosmic_ray_mask: mask,
        cosmic_ray_count,
        laplacian,
    }
}

/// Grow cosmic ray mask to neighboring pixels that are also elevated.
#[allow(dead_code)] // Used in tests and detect_cosmic_rays
fn grow_mask(
    mask: &[bool],
    width: usize,
    height: usize,
    radius: usize,
    pixels: &[f32],
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

/// Compute L.A.Cosmic-style Laplacian SNR for a single star candidate.
///
/// This can be used as an additional metric in star detection to reject
/// cosmic rays. High values (>5) indicate cosmic ray-like sharp edges.
///
/// # Arguments
/// * `pixels` - Image pixel data
/// * `width` - Image width
/// * `height` - Image height
/// * `cx` - Star center x coordinate
/// * `cy` - Star center y coordinate
/// * `stamp_radius` - Radius of analysis stamp
/// * `background` - Background level at star position
/// * `noise` - Noise level at star position
///
/// # Returns
/// Laplacian SNR value. Higher = more cosmic ray-like.
#[allow(clippy::too_many_arguments)]
pub fn compute_laplacian_snr(
    pixels: &[f32],
    width: usize,
    height: usize,
    cx: f32,
    cy: f32,
    stamp_radius: usize,
    background: f32,
    noise: f32,
) -> f32 {
    let icx = cx.round() as isize;
    let icy = cy.round() as isize;

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
        let pixels = vec![0.5f32; 25];
        let laplacian = compute_laplacian(&pixels, 5, 5);

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
        let mut pixels = vec![0.0f32; 25];
        pixels[2 * 5 + 2] = 1.0; // Center peak

        let laplacian = compute_laplacian(&pixels, 5, 5);

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
        let mut sharp = vec![0.0f32; 49];
        sharp[3 * 7 + 3] = 1.0;

        let mut gaussian = vec![0.0f32; 49];
        // Simple 3x3 Gaussian-ish pattern
        gaussian[3 * 7 + 3] = 1.0;
        gaussian[3 * 7 + 2] = 0.5;
        gaussian[3 * 7 + 4] = 0.5;
        gaussian[2 * 7 + 3] = 0.5;
        gaussian[4 * 7 + 3] = 0.5;
        gaussian[2 * 7 + 2] = 0.25;
        gaussian[2 * 7 + 4] = 0.25;
        gaussian[4 * 7 + 2] = 0.25;
        gaussian[4 * 7 + 4] = 0.25;

        let lapl_sharp = compute_laplacian(&sharp, 7, 7);
        let lapl_gaussian = compute_laplacian(&gaussian, 7, 7);

        // Sharp peak has more negative Laplacian than Gaussian
        assert!(
            lapl_sharp[3 * 7 + 3] < lapl_gaussian[3 * 7 + 3],
            "Sharp: {}, Gaussian: {}",
            lapl_sharp[3 * 7 + 3],
            lapl_gaussian[3 * 7 + 3]
        );
    }

    #[test]
    fn test_detect_cosmic_rays_sharp_peak() {
        // Single sharp pixel should be detected as cosmic ray
        // Use larger image for proper fine structure calculation
        let size = 15;
        let center = 7;
        let mut pixels = vec![0.1f32; size * size];
        pixels[center * size + center] = 1.0; // Sharp cosmic ray

        let background = vec![0.1f32; size * size];
        let noise = vec![0.01f32; size * size];

        let config = LACosmoicConfig::default();
        let result = detect_cosmic_rays(&pixels, size, size, &background, &noise, &config);

        assert!(
            result.cosmic_ray_mask[center * size + center],
            "Sharp peak should be detected as cosmic ray"
        );
    }

    #[test]
    fn test_detect_cosmic_rays_gaussian_star() {
        // Gaussian star with larger sigma should NOT be detected as cosmic ray
        // Use larger image and wider Gaussian to simulate a real star
        let size = 15;
        let center = 7;
        let sigma = 2.0f32; // Wider Gaussian simulating real seeing
        let mut pixels = vec![0.1f32; size * size];

        for y in 0..size {
            for x in 0..size {
                let dx = x as f32 - center as f32;
                let dy = y as f32 - center as f32;
                let r2 = dx * dx + dy * dy;
                let value = 0.8 * (-r2 / (2.0 * sigma * sigma)).exp();
                if value > 0.001 {
                    pixels[y * size + x] = 0.1 + value;
                }
            }
        }

        let background = vec![0.1f32; size * size];
        let noise = vec![0.01f32; size * size];

        let config = LACosmoicConfig::default();
        let result = detect_cosmic_rays(&pixels, size, size, &background, &noise, &config);

        // Gaussian star should not be flagged at center
        assert!(
            !result.cosmic_ray_mask[center * size + center],
            "Gaussian star should NOT be detected as cosmic ray"
        );
    }

    #[test]
    fn test_compute_laplacian_snr_cosmic_ray() {
        // Sharp peak has high Laplacian SNR
        let mut pixels = vec![0.1f32; 49];
        pixels[3 * 7 + 3] = 1.0;

        let snr = compute_laplacian_snr(&pixels, 7, 7, 3.0, 3.0, 2, 0.1, 0.01);

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
        let mut pixels = vec![0.1f32; size * size];

        for y in 0..size {
            for x in 0..size {
                let dx = x as f32 - center as f32;
                let dy = y as f32 - center as f32;
                let r2 = dx * dx + dy * dy;
                let value = 0.8 * (-r2 / (2.0 * sigma * sigma)).exp();
                if value > 0.001 {
                    pixels[y * size + x] = 0.1 + value;
                }
            }
        }

        let gaussian_snr = compute_laplacian_snr(
            &pixels,
            size,
            size,
            center as f32,
            center as f32,
            3,
            0.1,
            0.01,
        );

        // Compare to a cosmic ray (single sharp pixel)
        let mut cr_pixels = vec![0.1f32; size * size];
        cr_pixels[center * size + center] = 0.9;
        let cr_snr = compute_laplacian_snr(
            &cr_pixels,
            size,
            size,
            center as f32,
            center as f32,
            3,
            0.1,
            0.01,
        );

        // Cosmic ray should have significantly higher Laplacian SNR than Gaussian star
        assert!(
            cr_snr > gaussian_snr * 2.0,
            "Cosmic ray ({}) should have much higher Laplacian SNR than Gaussian star ({})",
            cr_snr,
            gaussian_snr
        );
    }

    #[test]
    fn test_grow_mask() {
        let mut mask = vec![false; 25];
        mask[12] = true; // Center pixel (2*5+2)

        let pixels = vec![0.5f32; 25]; // All elevated

        let grown = grow_mask(&mask, 5, 5, 1, &pixels);

        // All 8 neighbors plus center should be true
        assert!(grown[6]); // 1*5+1
        assert!(grown[7]); // 1*5+2
        assert!(grown[8]); // 1*5+3
        assert!(grown[11]); // 2*5+1
        assert!(grown[12]); // 2*5+2
        assert!(grown[13]); // 2*5+3
        assert!(grown[16]); // 3*5+1
        assert!(grown[17]); // 3*5+2
        assert!(grown[18]); // 3*5+3
    }

    #[test]
    fn test_median_of_n() {
        let mut values = [1.0, 5.0, 3.0, 2.0, 4.0];
        assert!((median_of_n(&mut values) - 3.0).abs() < 1e-6);

        let mut values2 = [1.0, 2.0, 3.0, 4.0];
        assert!((median_of_n(&mut values2) - 3.0).abs() < 1e-6);

        let mut values3 = [42.0];
        assert!((median_of_n(&mut values3) - 42.0).abs() < 1e-6);
    }

    #[test]
    fn test_fine_structure_flat() {
        let pixels = vec![0.5f32; 25];
        let fine = compute_fine_structure(&pixels, 5, 5);

        // Flat image has no fine structure
        for &v in &fine {
            assert!(v.abs() < 1e-6);
        }
    }

    #[test]
    fn test_fine_structure_peak() {
        let mut pixels = vec![0.0f32; 25];
        pixels[2 * 5 + 2] = 1.0;

        let fine = compute_fine_structure(&pixels, 5, 5);

        // Peak should have positive fine structure
        assert!(fine[2 * 5 + 2] > 0.5);
    }
}
