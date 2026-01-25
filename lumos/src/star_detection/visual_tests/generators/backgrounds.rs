//! Background generators for synthetic test images.
//!
//! Provides various background patterns:
//! - Uniform
//! - Linear gradients
//! - Radial vignette
//! - Nebula-like structures
//! - Amplifier glow (corner brightening)

use std::f32::consts::PI;

/// Add uniform background to image.
pub fn add_uniform_background(pixels: &mut [f32], level: f32) {
    for p in pixels.iter_mut() {
        *p += level;
    }
}

/// Add linear gradient background.
///
/// # Arguments
/// * `pixels` - Mutable pixel buffer
/// * `width`, `height` - Image dimensions
/// * `level_start` - Background level at top-left
/// * `level_end` - Background level at bottom-right
/// * `angle` - Gradient direction in radians (0 = horizontal left-to-right)
pub fn add_gradient_background(
    pixels: &mut [f32],
    width: usize,
    height: usize,
    level_start: f32,
    level_end: f32,
    angle: f32,
) {
    let cos_a = angle.cos();
    let sin_a = angle.sin();

    // Project diagonal to get max distance along gradient direction
    let max_dist = (width as f32 * cos_a.abs() + height as f32 * sin_a.abs()).max(1.0);

    for y in 0..height {
        for x in 0..width {
            let dist = x as f32 * cos_a + y as f32 * sin_a;
            let t = (dist / max_dist).clamp(0.0, 1.0);
            let level = level_start + (level_end - level_start) * t;
            pixels[y * width + x] += level;
        }
    }
}

/// Add radial vignette (darker corners).
///
/// # Arguments
/// * `pixels` - Mutable pixel buffer
/// * `width`, `height` - Image dimensions
/// * `center_level` - Background level at image center
/// * `edge_level` - Background level at corners
/// * `falloff` - Power of radial falloff (1.0 = linear, 2.0 = quadratic)
pub fn add_vignette_background(
    pixels: &mut [f32],
    width: usize,
    height: usize,
    center_level: f32,
    edge_level: f32,
    falloff: f32,
) {
    let cx = width as f32 / 2.0;
    let cy = height as f32 / 2.0;
    let max_r = (cx * cx + cy * cy).sqrt();

    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let r = (dx * dx + dy * dy).sqrt();
            let t = (r / max_r).powf(falloff);
            let level = center_level + (edge_level - center_level) * t;
            pixels[y * width + x] += level;
        }
    }
}

/// Add amplifier glow (corner brightening, typical in CCDs).
///
/// Creates a gradient from the specified corner that fades out.
///
/// # Arguments
/// * `pixels` - Mutable pixel buffer
/// * `width`, `height` - Image dimensions
/// * `corner` - Which corner (0=TL, 1=TR, 2=BL, 3=BR)
/// * `amplitude` - Maximum glow brightness
/// * `decay_scale` - Distance scale for exponential decay (in pixels)
pub fn add_amp_glow(
    pixels: &mut [f32],
    width: usize,
    height: usize,
    corner: usize,
    amplitude: f32,
    decay_scale: f32,
) {
    let (glow_x, glow_y) = match corner {
        0 => (0.0, 0.0),                                // Top-left
        1 => (width as f32 - 1.0, 0.0),                 // Top-right
        2 => (0.0, height as f32 - 1.0),                // Bottom-left
        _ => (width as f32 - 1.0, height as f32 - 1.0), // Bottom-right
    };

    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - glow_x;
            let dy = y as f32 - glow_y;
            let dist = (dx * dx + dy * dy).sqrt();
            let glow = amplitude * (-dist / decay_scale).exp();
            pixels[y * width + x] += glow;
        }
    }
}

/// Configuration for nebula-like background structure.
#[derive(Debug, Clone)]
pub struct NebulaConfig {
    /// Center X position (fraction of image width)
    pub center_x: f32,
    /// Center Y position (fraction of image height)
    pub center_y: f32,
    /// Radius as fraction of image diagonal
    pub radius: f32,
    /// Peak brightness
    pub amplitude: f32,
    /// Edge softness (higher = softer edges)
    pub softness: f32,
    /// Ellipticity (1.0 = circular)
    pub aspect_ratio: f32,
    /// Rotation angle in radians
    pub angle: f32,
}

impl Default for NebulaConfig {
    fn default() -> Self {
        Self {
            center_x: 0.5,
            center_y: 0.5,
            radius: 0.3,
            amplitude: 0.2,
            softness: 2.0,
            aspect_ratio: 1.0,
            angle: 0.0,
        }
    }
}

/// Add nebula-like diffuse background structure.
///
/// Creates an elliptical Gaussian-like bright region to simulate
/// emission nebulae or light pollution gradients.
pub fn add_nebula_background(
    pixels: &mut [f32],
    width: usize,
    height: usize,
    config: &NebulaConfig,
) {
    let cx = config.center_x * width as f32;
    let cy = config.center_y * height as f32;
    let diag = ((width * width + height * height) as f32).sqrt();
    let radius = config.radius * diag;
    let radius_sq = radius * radius;

    let cos_a = config.angle.cos();
    let sin_a = config.angle.sin();

    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;

            // Rotate and scale for ellipticity
            let dx_rot = dx * cos_a + dy * sin_a;
            let dy_rot = (-dx * sin_a + dy * cos_a) / config.aspect_ratio;

            let r_sq = dx_rot * dx_rot + dy_rot * dy_rot;
            let t = r_sq / radius_sq;

            // Smooth falloff with configurable softness
            let falloff = (-t * config.softness).exp();
            pixels[y * width + x] += config.amplitude * falloff;
        }
    }
}

/// Add multiple smaller nebula patches (simulates complex nebula structure).
pub fn add_complex_nebula(
    pixels: &mut [f32],
    width: usize,
    height: usize,
    base_amplitude: f32,
    seed: u64,
) {
    // Simple LCG for reproducible randomness
    let mut rng = seed;
    let next_f32 = |s: &mut u64| -> f32 {
        *s = s.wrapping_mul(6364136223846793005).wrapping_add(1);
        (*s >> 33) as f32 / (1u64 << 31) as f32
    };

    // Add 3-5 overlapping nebula patches
    let num_patches = 3 + (next_f32(&mut rng) * 3.0) as usize;

    for _ in 0..num_patches {
        let config = NebulaConfig {
            center_x: 0.2 + next_f32(&mut rng) * 0.6,
            center_y: 0.2 + next_f32(&mut rng) * 0.6,
            radius: 0.1 + next_f32(&mut rng) * 0.2,
            amplitude: base_amplitude * (0.3 + next_f32(&mut rng) * 0.7),
            softness: 1.5 + next_f32(&mut rng) * 2.0,
            aspect_ratio: 0.5 + next_f32(&mut rng) * 0.5,
            angle: next_f32(&mut rng) * PI,
        };
        add_nebula_background(pixels, width, height, &config);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uniform_background() {
        let mut pixels = vec![0.0f32; 64 * 64];
        add_uniform_background(&mut pixels, 0.1);

        for &p in &pixels {
            assert!((p - 0.1).abs() < 0.001);
        }
    }

    #[test]
    fn test_gradient_horizontal() {
        let width = 64;
        let height = 64;
        let mut pixels = vec![0.0f32; width * height];

        add_gradient_background(&mut pixels, width, height, 0.0, 1.0, 0.0);

        // Left edge should be ~0, right edge should be ~1
        assert!(pixels[32 * width] < 0.1);
        assert!(pixels[32 * width + width - 1] > 0.9);
    }

    #[test]
    fn test_vignette_center_brighter() {
        let width = 64;
        let height = 64;
        let mut pixels = vec![0.0f32; width * height];

        add_vignette_background(&mut pixels, width, height, 0.5, 0.1, 2.0);

        // Center should be brightest
        let center = pixels[32 * width + 32];
        let corner = pixels[0];

        assert!(
            center > corner,
            "Center {} should be > corner {}",
            center,
            corner
        );
    }

    #[test]
    fn test_amp_glow_decays() {
        let width = 64;
        let height = 64;
        let mut pixels = vec![0.0f32; width * height];

        add_amp_glow(&mut pixels, width, height, 0, 0.5, 20.0);

        // Top-left corner should be brightest
        let corner = pixels[0];
        let center = pixels[32 * width + 32];

        assert!(
            corner > center,
            "Corner {} should be > center {}",
            corner,
            center
        );
    }
}
