//! Background generators for synthetic test images.
//!
//! Provides various background patterns:
//! - Uniform
//! - Linear gradients
//! - Radial vignette
//! - Nebula-like structures
//! - Amplifier glow (corner brightening)

use glam::Vec2;

use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;

/// Add uniform background to image.
pub(super) fn add_uniform_background(pixels: &mut [f32], level: f32) {
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
pub(super) fn add_gradient_background(
    pixels: &mut [f32],
    size: Size2us,
    level_start: f32,
    level_end: f32,
    angle: f32,
) {
    let cos_a = angle.cos();
    let sin_a = angle.sin();

    // Project diagonal to get max distance along gradient direction
    let max_dist = (size.width as f32 * cos_a.abs() + size.height as f32 * sin_a.abs()).max(1.0);

    for y in 0..size.height {
        for x in 0..size.width {
            let dist = x as f32 * cos_a + y as f32 * sin_a;
            let t = (dist / max_dist).clamp(0.0, 1.0);
            let level = level_start + (level_end - level_start) * t;
            pixels[size.index_of(Vec2us::new(x, y))] += level;
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
pub(super) fn add_vignette_background(
    pixels: &mut [f32],
    size: Size2us,
    center_level: f32,
    edge_level: f32,
    falloff: f32,
) {
    let center = Vec2::new(size.width as f32 / 2.0, size.height as f32 / 2.0);
    let max_r = center.length();

    for y in 0..size.height {
        for x in 0..size.width {
            let pixel_pos = Vec2::new(x as f32, y as f32);
            let r = pixel_pos.distance(center);
            let t = (r / max_r).powf(falloff);
            let level = center_level + (edge_level - center_level) * t;
            pixels[size.index_of(Vec2us::new(x, y))] += level;
        }
    }
}

/// Configuration for nebula-like background structure.
#[derive(Debug, Clone)]
pub(crate) struct NebulaConfig {
    /// Center position (fraction of image width/height, 0.0-1.0)
    pub(crate) center: Vec2,
    /// Radius as fraction of image diagonal
    pub(crate) radius: f32,
    /// Peak brightness
    pub(crate) amplitude: f32,
    /// Edge softness (higher = softer edges)
    pub(crate) softness: f32,
    /// Ellipticity (1.0 = circular)
    pub(crate) aspect_ratio: f32,
    /// Rotation angle in radians
    pub(crate) angle: f32,
}

impl Default for NebulaConfig {
    fn default() -> Self {
        Self {
            center: Vec2::splat(0.5),
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
pub(super) fn add_nebula_background(pixels: &mut [f32], size: Size2us, config: &NebulaConfig) {
    let cx = config.center.x * size.width as f32;
    let cy = config.center.y * size.height as f32;
    let diag = ((size.width * size.width + size.height * size.height) as f32).sqrt();
    let radius = config.radius * diag;
    let radius_sq = radius * radius;

    let cos_a = config.angle.cos();
    let sin_a = config.angle.sin();

    for y in 0..size.height {
        for x in 0..size.width {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;

            // Rotate and scale for ellipticity
            let dx_rot = dx * cos_a + dy * sin_a;
            let dy_rot = (-dx * sin_a + dy * cos_a) / config.aspect_ratio;

            let r_sq = dx_rot * dx_rot + dy_rot * dy_rot;
            let t = r_sq / radius_sq;

            // Smooth falloff with configurable softness
            let falloff = (-t * config.softness).exp();
            pixels[size.index_of(Vec2us::new(x, y))] += config.amplitude * falloff;
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::synthetic::backgrounds::*;

    #[test]
    fn uniform_background() {
        let mut pixels = vec![0.0f32; 64 * 64];
        add_uniform_background(&mut pixels, 0.1);

        for &p in &pixels {
            assert!((p - 0.1).abs() < 0.001);
        }
    }

    #[test]
    fn gradient_horizontal() {
        let width = 64;
        let height = 64;
        let mut pixels = vec![0.0f32; width * height];

        add_gradient_background(&mut pixels, Size2us::new(width, height), 0.0, 1.0, 0.0);

        // Left edge should be ~0, right edge should be ~1
        assert!(pixels[32 * width] < 0.1);
        assert!(pixels[32 * width + width - 1] > 0.9);
    }

    #[test]
    fn vignette_center_brighter() {
        let width = 64;
        let height = 64;
        let mut pixels = vec![0.0f32; width * height];

        add_vignette_background(&mut pixels, Size2us::new(width, height), 0.5, 0.1, 2.0);

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
}
