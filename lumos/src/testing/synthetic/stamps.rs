//! Small star stamp generation for centroid and profile fitting tests.
//!
//! Provides functions to generate small image stamps containing single stars
//! with various PSF profiles (Gaussian, Moffat, elliptical).

use crate::common::Buffer2;

/// Generate a Gaussian star stamp.
///
/// Creates a small image with a single Gaussian star at the specified position.
///
/// # Arguments
/// * `width` - Stamp width in pixels
/// * `height` - Stamp height in pixels
/// * `cx` - Star center X position (can be sub-pixel)
/// * `cy` - Star center Y position (can be sub-pixel)
/// * `sigma` - Gaussian sigma (FWHM ≈ 2.355 * sigma)
/// * `amplitude` - Peak amplitude above background
/// * `background` - Background level
pub fn gaussian(
    width: usize,
    height: usize,
    cx: f32,
    cy: f32,
    sigma: f32,
    amplitude: f32,
    background: f32,
) -> Buffer2<f32> {
    let mut pixels = vec![background; width * height];
    let two_sigma_sq = 2.0 * sigma * sigma;

    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let r2 = dx * dx + dy * dy;
            let value = amplitude * (-r2 / two_sigma_sq).exp();
            pixels[y * width + x] += value;
        }
    }
    Buffer2::new(width, height, pixels)
}

/// Generate a Moffat star stamp.
///
/// Creates a small image with a single Moffat profile star.
/// Moffat profiles have extended wings compared to Gaussian, better matching
/// real atmospheric seeing.
///
/// # Arguments
/// * `width` - Stamp width in pixels
/// * `height` - Stamp height in pixels
/// * `cx` - Star center X position
/// * `cy` - Star center Y position
/// * `alpha` - Moffat alpha parameter (core width)
/// * `beta` - Moffat beta parameter (wing strength, typically 2.5-4.0)
/// * `amplitude` - Peak amplitude above background
/// * `background` - Background level
#[allow(clippy::too_many_arguments)]
pub fn moffat(
    width: usize,
    height: usize,
    cx: f32,
    cy: f32,
    alpha: f32,
    beta: f32,
    amplitude: f32,
    background: f32,
) -> Buffer2<f32> {
    let mut pixels = vec![background; width * height];
    let alpha_sq = alpha * alpha;

    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let r2 = dx * dx + dy * dy;
            let value = amplitude * (1.0 + r2 / alpha_sq).powf(-beta);
            pixels[y * width + x] += value;
        }
    }
    Buffer2::new(width, height, pixels)
}

/// Generate an elliptical Gaussian star stamp.
///
/// Creates a star with different widths along major and minor axes,
/// simulating tracking errors or optical aberrations.
///
/// # Arguments
/// * `width` - Stamp width in pixels
/// * `height` - Stamp height in pixels
/// * `cx` - Star center X position
/// * `cy` - Star center Y position
/// * `sigma_major` - Sigma along major axis
/// * `sigma_minor` - Sigma along minor axis
/// * `angle` - Rotation angle of major axis in radians
/// * `amplitude` - Peak amplitude above background
/// * `background` - Background level
#[allow(clippy::too_many_arguments)]
pub fn elliptical(
    width: usize,
    height: usize,
    cx: f32,
    cy: f32,
    sigma_major: f32,
    sigma_minor: f32,
    angle: f32,
    amplitude: f32,
    background: f32,
) -> Buffer2<f32> {
    let mut pixels = vec![background; width * height];
    let cos_a = angle.cos();
    let sin_a = angle.sin();
    let two_major_sq = 2.0 * sigma_major * sigma_major;
    let two_minor_sq = 2.0 * sigma_minor * sigma_minor;

    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            // Rotate to ellipse coordinate system
            let dx_rot = dx * cos_a + dy * sin_a;
            let dy_rot = -dx * sin_a + dy * cos_a;
            let value = amplitude
                * (-dx_rot * dx_rot / two_major_sq - dy_rot * dy_rot / two_minor_sq).exp();
            pixels[y * width + x] += value;
        }
    }
    Buffer2::new(width, height, pixels)
}

/// Generate a star field stamp with multiple Gaussian stars.
///
/// Useful for benchmarking centroid computation on multiple stars.
///
/// # Arguments
/// * `width` - Stamp width in pixels
/// * `height` - Stamp height in pixels
/// * `num_stars` - Number of stars to generate
/// * `sigma` - Gaussian sigma for all stars
/// * `background` - Background level
/// * `seed` - Random seed for reproducibility
pub fn star_field(
    width: usize,
    height: usize,
    num_stars: usize,
    sigma: f32,
    background: f32,
    seed: u64,
) -> (Buffer2<f32>, Vec<(f32, f32)>) {
    let mut pixels = vec![background; width * height];
    let mut positions = Vec::with_capacity(num_stars);
    let mut rng = crate::testing::TestRng::new(seed);

    let margin = (sigma * 4.0).ceil() as usize;
    let two_sigma_sq = 2.0 * sigma * sigma;

    for _ in 0..num_stars {
        // Random position with margin from edges
        let cx =
            margin as f32 + rng.next_f32() * (width - 2 * margin) as f32 + rng.next_f32() * 0.5;
        let cy =
            margin as f32 + rng.next_f32() * (height - 2 * margin) as f32 + rng.next_f32() * 0.5;
        let brightness = 0.5 + rng.next_f32() * 0.5;

        // Add Gaussian star
        let radius = (sigma * 4.0).ceil() as i32;
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                let x = (cx as i32 + dx) as usize;
                let y = (cy as i32 + dy) as usize;
                if x < width && y < height {
                    let fx = x as f32 - cx;
                    let fy = y as f32 - cy;
                    let r2 = fx * fx + fy * fy;
                    let value = brightness * (-r2 / two_sigma_sq).exp();
                    pixels[y * width + x] += value;
                }
            }
        }
        positions.push((cx, cy));
    }

    (Buffer2::new(width, height, pixels), positions)
}

/// Generate a single Gaussian spot (useful for phase correlation tests).
///
/// Simpler interface for creating a centered Gaussian spot.
pub fn gaussian_spot(
    width: usize,
    height: usize,
    spot_x: f32,
    spot_y: f32,
    sigma: f32,
) -> Buffer2<f32> {
    gaussian(width, height, spot_x, spot_y, sigma, 1.0, 0.0)
}

/// Generate multiple Gaussian spots for feature-rich test images.
pub fn multi_gaussian_spots(
    width: usize,
    height: usize,
    spots: &[(f32, f32)],
    sigma: f32,
) -> Buffer2<f32> {
    let mut pixels = vec![0.0f32; width * height];
    let two_sigma_sq = 2.0 * sigma * sigma;

    for &(cx, cy) in spots {
        for y in 0..height {
            for x in 0..width {
                let dx = x as f32 - cx;
                let dy = y as f32 - cy;
                let r2 = dx * dx + dy * dy;
                let value = (-r2 / two_sigma_sq).exp();
                pixels[y * width + x] += value;
            }
        }
    }
    Buffer2::new(width, height, pixels)
}

/// Generate a large star field for benchmarking detection algorithms.
///
/// This is optimized for benchmark use - generates stars with varying FWHM
/// and deterministic positions, plus adds background noise.
///
/// # Arguments
/// * `width` - Image width
/// * `height` - Image height
/// * `num_stars` - Number of stars to generate
/// * `background` - Background level
/// * `noise_amplitude` - Amplitude of deterministic noise
/// * `seed` - Random seed for reproducibility
pub fn benchmark_star_field(
    width: usize,
    height: usize,
    num_stars: usize,
    background: f32,
    noise_amplitude: f32,
    seed: u64,
) -> Buffer2<f32> {
    let mut pixels = vec![background; width * height];

    // Add deterministic noise using hash
    for (i, p) in pixels.iter_mut().enumerate() {
        let hash =
            ((i as u64).wrapping_mul(2654435761).wrapping_add(seed)) as f32 / (u64::MAX as f32);
        *p += (hash - 0.5) * 2.0 * noise_amplitude;
    }

    // Add synthetic stars
    let mut rng = crate::testing::TestRng::new(seed);

    let margin = 15;
    for _ in 0..num_stars {
        let cx = margin + ((rng.next_u64() >> 33) as usize % (width - 2 * margin));
        let cy = margin + ((rng.next_u64() >> 33) as usize % (height - 2 * margin));
        let brightness = 0.5 + rng.next_f32() * 0.5;
        let sigma = 1.5 + rng.next_f32();
        let two_sigma_sq = 2.0 * sigma * sigma;

        // Render star with limited radius for efficiency
        let radius = (sigma * 4.0).ceil() as i32;
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                let x = (cx as i32 + dx) as usize;
                let y = (cy as i32 + dy) as usize;
                if x < width && y < height {
                    let r2 = (dx * dx + dy * dy) as f32;
                    let value = brightness * (-r2 / two_sigma_sq).exp();
                    pixels[y * width + x] += value;
                }
            }
        }
    }

    Buffer2::new(width, height, pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gaussian_stamp() {
        let stamp = gaussian(21, 21, 10.0, 10.0, 2.0, 1.0, 0.1);
        assert_eq!(stamp.width(), 21);
        assert_eq!(stamp.height(), 21);
        // Peak should be at center
        assert!(stamp[(10, 10)] > stamp[(0, 0)]);
        // Peak value should be amplitude + background
        assert!((stamp[(10, 10)] - 1.1).abs() < 0.01);
    }

    #[test]
    fn test_moffat_stamp() {
        let stamp = moffat(21, 21, 10.0, 10.0, 2.5, 2.5, 1.0, 0.1);
        // Peak should be at center
        assert!(stamp[(10, 10)] > stamp[(0, 0)]);
        // Moffat has wider wings than Gaussian
        let gaussian_stamp = gaussian(21, 21, 10.0, 10.0, 2.0, 1.0, 0.1);
        // At edges, Moffat should have higher values (wider wings)
        assert!(stamp[(5, 10)] > gaussian_stamp[(5, 10)] - 0.1);
    }

    #[test]
    fn test_elliptical_stamp() {
        let stamp = elliptical(31, 31, 15.0, 15.0, 4.0, 2.0, 0.0, 1.0, 0.0);
        // Peak at center
        assert!(stamp[(15, 15)] > 0.9);
        // Should be wider along X (major axis at angle=0)
        assert!(stamp[(19, 15)] > stamp[(15, 19)]);
    }

    #[test]
    fn test_star_field() {
        let (pixels, positions) = star_field(256, 256, 10, 2.0, 0.1, 42);
        assert_eq!(positions.len(), 10);
        // All positions should be within bounds with margin
        for (x, y) in &positions {
            assert!(*x > 8.0 && *x < 248.0);
            assert!(*y > 8.0 && *y < 248.0);
        }
        // Pixels at star positions should be brighter than background
        for (x, y) in &positions {
            let px = *x as usize;
            let py = *y as usize;
            assert!(pixels[(px, py)] > 0.2);
        }
    }

    #[test]
    fn test_gaussian_spot() {
        let spot = gaussian_spot(64, 64, 32.0, 32.0, 10.0);
        assert!((spot[(32, 32)] - 1.0).abs() < 0.01);
        assert!(spot[(0, 0)] < 0.1);
    }

    #[test]
    fn test_multi_gaussian_spots() {
        let spots = vec![(16.0, 16.0), (48.0, 48.0)];
        let img = multi_gaussian_spots(64, 64, &spots, 5.0);
        // Both spots should be bright
        assert!(img[(16, 16)] > 0.9);
        assert!(img[(48, 48)] > 0.9);
    }
}
