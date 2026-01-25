//! Star profile rendering functions.
//!
//! Provides various PSF models for generating synthetic stars:
//! - Gaussian (ideal seeing)
//! - Moffat (realistic atmospheric PSF)
//! - Elliptical Gaussian (tracking errors)
//! - Saturated stars (flat-topped profiles)

use crate::star_detection::constants::FWHM_TO_SIGMA;

/// Render a circular Gaussian star profile.
///
/// # Arguments
/// * `pixels` - Mutable pixel buffer to add star to
/// * `width` - Image width
/// * `x`, `y` - Star center position (sub-pixel)
/// * `sigma` - Gaussian sigma (FWHM = 2.355 * sigma)
/// * `amplitude` - Peak brightness above background
pub fn render_gaussian_star(
    pixels: &mut [f32],
    width: usize,
    x: f32,
    y: f32,
    sigma: f32,
    amplitude: f32,
) {
    let height = pixels.len() / width;
    let radius = (4.0 * sigma).ceil() as i32;
    let two_sigma_sq = 2.0 * sigma * sigma;

    let cx = x.round() as i32;
    let cy = y.round() as i32;

    let x_min = (cx - radius).max(0) as usize;
    let x_max = ((cx + radius) as usize).min(width - 1);
    let y_min = (cy - radius).max(0) as usize;
    let y_max = ((cy + radius) as usize).min(height - 1);

    for py in y_min..=y_max {
        for px in x_min..=x_max {
            let dx = px as f32 - x;
            let dy = py as f32 - y;
            let r_sq = dx * dx + dy * dy;
            let value = amplitude * (-r_sq / two_sigma_sq).exp();
            pixels[py * width + px] += value;
        }
    }
}

/// Render a Moffat profile star (more realistic atmospheric PSF).
///
/// The Moffat profile has extended wings compared to Gaussian:
/// I(r) = I0 * (1 + (r/alpha)^2)^(-beta)
///
/// FWHM = 2 * alpha * sqrt(2^(1/beta) - 1)
///
/// # Arguments
/// * `pixels` - Mutable pixel buffer
/// * `width` - Image width
/// * `x`, `y` - Star center position
/// * `alpha` - Scale parameter
/// * `beta` - Shape parameter (typical: 2.5-4.0)
/// * `amplitude` - Peak brightness
pub fn render_moffat_star(
    pixels: &mut [f32],
    width: usize,
    x: f32,
    y: f32,
    alpha: f32,
    beta: f32,
    amplitude: f32,
) {
    let height = pixels.len() / width;
    // Moffat has extended wings, use larger radius
    let radius = (8.0 * alpha).ceil() as i32;
    let alpha_sq = alpha * alpha;

    let cx = x.round() as i32;
    let cy = y.round() as i32;

    let x_min = (cx - radius).max(0) as usize;
    let x_max = ((cx + radius) as usize).min(width - 1);
    let y_min = (cy - radius).max(0) as usize;
    let y_max = ((cy + radius) as usize).min(height - 1);

    for py in y_min..=y_max {
        for px in x_min..=x_max {
            let dx = px as f32 - x;
            let dy = py as f32 - y;
            let r_sq = dx * dx + dy * dy;
            let value = amplitude * (1.0 + r_sq / alpha_sq).powf(-beta);
            pixels[py * width + px] += value;
        }
    }
}

/// Convert Moffat parameters to FWHM.
pub fn moffat_fwhm(alpha: f32, beta: f32) -> f32 {
    2.0 * alpha * (2.0f32.powf(1.0 / beta) - 1.0).sqrt()
}

/// Convert FWHM to Moffat alpha parameter (given beta).
pub fn fwhm_to_moffat_alpha(fwhm: f32, beta: f32) -> f32 {
    fwhm / (2.0 * (2.0f32.powf(1.0 / beta) - 1.0).sqrt())
}

/// Render an elliptical Gaussian star (simulates tracking errors).
///
/// # Arguments
/// * `pixels` - Mutable pixel buffer
/// * `width` - Image width
/// * `x`, `y` - Star center position
/// * `sigma_major` - Sigma along major axis
/// * `sigma_minor` - Sigma along minor axis
/// * `angle` - Rotation angle in radians (0 = major axis horizontal)
/// * `amplitude` - Peak brightness
#[allow(clippy::too_many_arguments)]
pub fn render_elliptical_star(
    pixels: &mut [f32],
    width: usize,
    x: f32,
    y: f32,
    sigma_major: f32,
    sigma_minor: f32,
    angle: f32,
    amplitude: f32,
) {
    let height = pixels.len() / width;
    let radius = (4.0 * sigma_major).ceil() as i32;

    let cos_a = angle.cos();
    let sin_a = angle.sin();
    let two_sigma_maj_sq = 2.0 * sigma_major * sigma_major;
    let two_sigma_min_sq = 2.0 * sigma_minor * sigma_minor;

    let cx = x.round() as i32;
    let cy = y.round() as i32;

    let x_min = (cx - radius).max(0) as usize;
    let x_max = ((cx + radius) as usize).min(width - 1);
    let y_min = (cy - radius).max(0) as usize;
    let y_max = ((cy + radius) as usize).min(height - 1);

    for py in y_min..=y_max {
        for px in x_min..=x_max {
            let dx = px as f32 - x;
            let dy = py as f32 - y;

            // Rotate coordinates to align with ellipse axes
            let dx_rot = dx * cos_a + dy * sin_a;
            let dy_rot = -dx * sin_a + dy * cos_a;

            let exponent = dx_rot * dx_rot / two_sigma_maj_sq + dy_rot * dy_rot / two_sigma_min_sq;
            let value = amplitude * (-exponent).exp();
            pixels[py * width + px] += value;
        }
    }
}

/// Compute eccentricity from sigma_major and sigma_minor.
pub fn compute_eccentricity(sigma_major: f32, sigma_minor: f32) -> f32 {
    let ratio = sigma_minor / sigma_major;
    (1.0 - ratio * ratio).sqrt()
}

/// Render a saturated star (flat-topped Gaussian).
///
/// Stars become saturated when pixel values exceed the detector's well capacity.
/// The profile is clipped at saturation_level.
///
/// # Arguments
/// * `pixels` - Mutable pixel buffer
/// * `width` - Image width
/// * `x`, `y` - Star center position
/// * `sigma` - Gaussian sigma
/// * `amplitude` - Peak brightness (before saturation)
/// * `saturation_level` - Maximum pixel value (typically 0.95-1.0)
pub fn render_saturated_star(
    pixels: &mut [f32],
    width: usize,
    x: f32,
    y: f32,
    sigma: f32,
    amplitude: f32,
    saturation_level: f32,
) {
    let height = pixels.len() / width;
    let radius = (4.0 * sigma).ceil() as i32;
    let two_sigma_sq = 2.0 * sigma * sigma;

    let cx = x.round() as i32;
    let cy = y.round() as i32;

    let x_min = (cx - radius).max(0) as usize;
    let x_max = ((cx + radius) as usize).min(width - 1);
    let y_min = (cy - radius).max(0) as usize;
    let y_max = ((cy + radius) as usize).min(height - 1);

    for py in y_min..=y_max {
        for px in x_min..=x_max {
            let dx = px as f32 - x;
            let dy = py as f32 - y;
            let r_sq = dx * dx + dy * dy;
            let value = amplitude * (-r_sq / two_sigma_sq).exp();
            let idx = py * width + px;
            pixels[idx] = (pixels[idx] + value).min(saturation_level);
        }
    }
}

/// Render a cosmic ray hit (very sharp single-pixel spike).
pub fn render_cosmic_ray(pixels: &mut [f32], width: usize, x: usize, y: usize, amplitude: f32) {
    let height = pixels.len() / width;
    if x < width && y < height {
        pixels[y * width + x] += amplitude;
    }
}

/// Render a cosmic ray with slight bleeding to neighbors.
pub fn render_cosmic_ray_extended(
    pixels: &mut [f32],
    width: usize,
    x: usize,
    y: usize,
    amplitude: f32,
) {
    let height = pixels.len() / width;
    if x >= width || y >= height {
        return;
    }

    // Central pixel gets most of the flux
    pixels[y * width + x] += amplitude;

    // Small fraction bleeds to neighbors
    let bleed = amplitude * 0.1;
    if x > 0 {
        pixels[y * width + x - 1] += bleed;
    }
    if x < width - 1 {
        pixels[y * width + x + 1] += bleed;
    }
    if y > 0 {
        pixels[(y - 1) * width + x] += bleed;
    }
    if y < height - 1 {
        pixels[(y + 1) * width + x] += bleed;
    }
}

/// Convert FWHM to Gaussian sigma.
pub fn fwhm_to_sigma(fwhm: f32) -> f32 {
    fwhm / FWHM_TO_SIGMA
}

/// Convert Gaussian sigma to FWHM.
pub fn sigma_to_fwhm(sigma: f32) -> f32 {
    sigma * FWHM_TO_SIGMA
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gaussian_peak_at_center() {
        let width = 64;
        let height = 64;
        let mut pixels = vec![0.0f32; width * height];

        render_gaussian_star(&mut pixels, width, 32.0, 32.0, 2.0, 1.0);

        // Peak should be at center
        let peak_idx = 32 * width + 32;
        assert!(pixels[peak_idx] > 0.9, "Peak should be near 1.0");

        // Far corner should be near 0
        assert!(pixels[0] < 0.001, "Corner should be near 0");
    }

    #[test]
    fn test_elliptical_has_elongation() {
        let width = 64;
        let height = 64;
        let mut pixels = vec![0.0f32; width * height];

        // Major axis horizontal
        render_elliptical_star(&mut pixels, width, 32.0, 32.0, 4.0, 2.0, 0.0, 1.0);

        // Check that horizontal extent > vertical extent
        let horiz_val = pixels[32 * width + 38]; // 6 pixels right
        let vert_val = pixels[38 * width + 32]; // 6 pixels down

        assert!(
            horiz_val > vert_val,
            "Horizontal should have more flux: {} vs {}",
            horiz_val,
            vert_val
        );
    }

    #[test]
    fn test_saturated_clipping() {
        let width = 64;
        let height = 64;
        let mut pixels = vec![0.0f32; width * height];

        render_saturated_star(&mut pixels, width, 32.0, 32.0, 2.0, 2.0, 0.95);

        // Center should be clipped to saturation level
        let peak_idx = 32 * width + 32;
        assert!(
            (pixels[peak_idx] - 0.95).abs() < 0.001,
            "Peak should be at saturation level"
        );
    }

    #[test]
    fn test_moffat_fwhm_conversion() {
        let beta = 2.5;
        let fwhm = 4.0;
        let alpha = fwhm_to_moffat_alpha(fwhm, beta);
        let recovered_fwhm = moffat_fwhm(alpha, beta);

        assert!(
            (recovered_fwhm - fwhm).abs() < 0.001,
            "FWHM conversion should be reversible"
        );
    }
}
