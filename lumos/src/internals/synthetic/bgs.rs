//! Background generators for synthetic test images: uniform, linear gradient, radial vignette
//! and nebula-like structure.

use glam::Vec2;
use imaginarium::Buffer2;

use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;

/// Add uniform background to image.
pub(super) fn add_uniform_bg(pixels: &mut [f32], level: f32) {
    for p in pixels.iter_mut() {
        *p += level;
    }
}

/// A linear gradient along `angle` (radians, 0 = left to right): `start` at the pixel the
/// direction leaves from and `end` at the pixel it reaches last — the first and last columns at
/// angle 0, as [`horizontal_gradient`](crate::internals::synthetic::patterns::horizontal_gradient)
/// spans them. A frame with no extent along the direction takes the midpoint.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Gradient {
    pub(crate) start: f32,
    pub(crate) end: f32,
    pub(crate) angle: f32,
}

impl Gradient {
    /// Add the gradient to `pixels`.
    pub(crate) fn add_to(self, pixels: &mut Buffer2<f32>) {
        let Self {
            start: level_start,
            end: level_end,
            angle,
        } = self;
        let size = Size2us::from(&*pixels);
        let cos_a = angle.cos();
        let sin_a = angle.sin();
        let project = |x: f32, y: f32| x * cos_a + y * sin_a;
        let (last_x, last_y) = ((size.width - 1) as f32, (size.height - 1) as f32);
        let corners = [
            project(0.0, 0.0),
            project(last_x, 0.0),
            project(0.0, last_y),
            project(last_x, last_y),
        ];
        let first = corners.into_iter().fold(f32::INFINITY, f32::min);
        let span = corners.into_iter().fold(f32::NEG_INFINITY, f32::max) - first;

        for y in 0..size.height {
            for x in 0..size.width {
                let t = if span > 0.0 {
                    ((project(x as f32, y as f32) - first) / span).clamp(0.0, 1.0)
                } else {
                    0.5
                };
                let level = level_start + (level_end - level_start) * t;
                pixels[size.idx_of(Vec2us::new(x, y))] += level;
            }
        }
    }
}

/// A radial vignette: `center` at the image centre, `edge` at the corners, and between them
/// the radius over the corner radius to the power `falloff` (1 linear, 2 quadratic).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Vignette {
    pub(crate) center: f32,
    pub(crate) edge: f32,
    pub(crate) falloff: f32,
}

impl Vignette {
    /// The level at pixel `(x, y)` of a `size` frame.
    pub(crate) fn at(self, size: Size2us, x: usize, y: usize) -> f32 {
        let centre = Vec2::new(size.width as f32 / 2.0, size.height as f32 / 2.0);
        let max_r = centre.length().max(1.0);
        let t = (Vec2::new(x as f32, y as f32).distance(centre) / max_r).powf(self.falloff);
        self.center + (self.edge - self.center) * t
    }

    /// Add the vignette to `pixels`.
    pub(crate) fn add_to(self, pixels: &mut Buffer2<f32>) {
        let size = Size2us::from(&*pixels);
        for y in 0..size.height {
            for x in 0..size.width {
                pixels[size.idx_of(Vec2us::new(x, y))] += self.at(size, x, y);
            }
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

impl NebulaConfig {
    /// Add nebula-like diffuse background structure to `pixels`.
    ///
    /// Creates an elliptical Gaussian-like bright region to simulate
    /// emission nebulae or light pollution gradients.
    pub(crate) fn add_to(&self, pixels: &mut Buffer2<f32>) {
        let size = Size2us::from(&*pixels);
        let cx = self.center.x * size.width as f32;
        let cy = self.center.y * size.height as f32;
        let diag = ((size.width * size.width + size.height * size.height) as f32).sqrt();
        let radius = self.radius * diag;
        let radius_sq = radius * radius;

        let cos_a = self.angle.cos();
        let sin_a = self.angle.sin();

        for y in 0..size.height {
            for x in 0..size.width {
                let dx = x as f32 - cx;
                let dy = y as f32 - cy;

                let dx_rot = dx * cos_a + dy * sin_a;
                let dy_rot = (-dx * sin_a + dy * cos_a) / self.aspect_ratio;

                let r_sq = dx_rot * dx_rot + dy_rot * dy_rot;
                let t = r_sq / radius_sq;

                let falloff = (-t * self.softness).exp();
                pixels[size.idx_of(Vec2us::new(x, y))] += self.amplitude * falloff;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::internals::assertions::assert_close;
    use crate::internals::synthetic::bgs::*;

    use crate::internals::synthetic::patterns;
    use std::f32::consts::FRAC_PI_2;

    #[test]
    fn uniform_bg() {
        let mut pixels = vec![0.25f32; 64 * 64];
        add_uniform_bg(&mut pixels, 0.5);
        assert!(pixels.iter().all(|&p| p == 0.75));
    }

    /// At angle 0 the gradient spans the first column to the last, as `horizontal_gradient` does:
    /// `t = x/63` in both, so the two agree bit for bit and reach both end levels exactly. At a
    /// right angle it runs down the rows instead, and a one-column-wide angle-0 frame has no
    /// extent to span, so it takes the midpoint.
    #[test]
    fn gradient_spans_its_end_levels() {
        let size = Size2us::new(64, 4);
        let mut pixels = Buffer2::new_default(64, 4);
        Gradient {
            start: 0.0,
            end: 1.0,
            angle: 0.0,
        }
        .add_to(&mut pixels);
        assert_eq!(pixels, patterns::horizontal_gradient(size, 0.0, 1.0));
        assert_eq!((pixels[0], pixels[63]), (0.0, 1.0));

        // At π/2 the f32 cosine is −4.371e-8, not 0, so a row's 63 columns tilt it by
        // 63·4.371e-8 = 2.754e-6, and the projection, the offset, the division and the scale
        // round once each, half an ulp of 3 (1.2e-7) at most: 3.23e-6 off the row's level `y`.
        let mut pixels = Buffer2::new_default(64, 4);
        Gradient {
            start: 0.0,
            end: 3.0,
            angle: FRAC_PI_2,
        }
        .add_to(&mut pixels);
        for (idx, &value) in pixels.iter().enumerate() {
            assert_close!(value, (idx / 64) as f32, 3.23e-6, "pixel {idx}");
        }

        let mut pixels = Buffer2::new_default(1, 4);
        Gradient {
            start: 0.0,
            end: 1.0,
            angle: 0.0,
        }
        .add_to(&mut pixels);
        assert_eq!(pixels.pixels(), [0.5; 4]);
    }

    /// (32, 32) is the centre of a 64×64 frame, radius 0, so it reads `center_level` exactly. The
    /// corner (0, 0) is at the corner radius, `t = 1`, so it reads `0.5 + (0.1 − 0.5)`: two f32
    /// roundings of at most half an ulp of 0.4 each, ε/4 in all.
    #[test]
    fn vignette_reads_its_levels_at_centre_and_corner() {
        let mut pixels = Buffer2::new_default(64, 64);
        Vignette {
            center: 0.5,
            edge: 0.1,
            falloff: 2.0,
        }
        .add_to(&mut pixels);
        assert_eq!(pixels[32 * 64 + 32], 0.5);
        assert_close!(pixels[0], 0.1, f32::EPSILON / 4.0);
    }
}
