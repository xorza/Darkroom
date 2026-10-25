//! Star profile rendering for synthetic fixtures.
//!
//! A [`StarProfile`] is the analytic shape — Gaussian, elliptical Gaussian, or Moffat — and a
//! [`SyntheticStar`] binds one to a centre and a peak amplitude. This is the crate's only
//! definition of those profiles; [`PsfModel`](crate::internals::synthetic::camera::PsfModel)
//! layers flux normalization on top of it rather than re-deriving the math.
//!
//! A pixel records the profile's mean over its area, as a sensor whose pixels collect all the light
//! that falls on them does: [`StarPixels`]. [`SyntheticStar::value_at`] is the profile at one
//! point, for the shape alone.
//!
//! Two rendering modes, and the difference is load-bearing:
//!
//! - [`SyntheticStar::add_to`] visits only the pixels within [`StarProfile::radius`]. Populated
//!   scenes need this — rendering 400 stars into a 6K frame cannot afford a full-frame loop per
//!   star — and the truncation edge is far enough down the profile to be invisible to a detector.
//! - [`SyntheticStar::add_exact`] and [`SyntheticStar::stamp`] visit every pixel. Fitting tests
//!   need this: they assert that a fitter recovers the parameters that generated the data, so a
//!   truncated wing is a systematic error they would otherwise have to widen their tolerances
//!   to absorb.

use common::Buffer2;
use glam::Vec2;

use crate::math::pixel_gaussian::PixelGaussian;
use crate::math::pixel_quadrature::{AnalyticProfile, PixelQuadrature};
use common::Size2us;

/// How closely a pixel mean without a closed form is integrated, relative to the peak: far under
/// the f32 rounding of a sample, and under any tolerance a test asserts.
const PIXEL_MEAN_TOLERANCE: f64 = 1e-12;

/// The analytic shape of a star profile, parameterized by peak amplitude.
#[derive(Debug, Clone, Copy)]
pub(crate) enum StarProfile {
    /// Gaussian `exp(−½(x′²/σx² + y′²/σy²))` in its own axes `x′`, `y′`: round where the two σ
    /// agree ([`Self::gaussian`]), elongated as tracking error leaves a star where they differ.
    Gaussian {
        /// Sigma along the profile's own x axis, before the `angle` rotation, in pixels.
        sigma_x: f32,
        /// Sigma along the profile's own y axis, before the `angle` rotation, in pixels.
        sigma_y: f32,
        /// Rotation of those axes, in radians. Zero leaves them axis-aligned.
        angle: f32,
    },
    /// Moffat profile: `(1 + (r/α)²)^-β`. Models the extended atmospheric wings a Gaussian
    /// misses; `beta` is typically 2.5–4.0.
    Moffat {
        /// Scale parameter in pixels.
        alpha: f32,
        /// Shape parameter — lower means heavier wings.
        beta: f32,
    },
}

impl StarProfile {
    /// The round Gaussian of standard deviation `sigma`, in pixels (FWHM = 2.355σ).
    pub(crate) const fn gaussian(sigma: f32) -> Self {
        Self::Gaussian {
            sigma_x: sigma,
            sigma_y: sigma,
            angle: 0.0,
        }
    }

    /// Profile value at `(dx, dy)` pixels from the centre, as a fraction of the peak.
    pub(crate) fn shape_at(self, dx: f64, dy: f64) -> f64 {
        match self {
            Self::Gaussian {
                sigma_x,
                sigma_y,
                angle,
            } => {
                let (sigma_x, sigma_y) = (f64::from(sigma_x), f64::from(sigma_y));
                let (sin_a, cos_a) = f64::from(angle).sin_cos();
                let x_rot = dx * cos_a + dy * sin_a;
                let y_rot = -dx * sin_a + dy * cos_a;
                let exponent = x_rot * x_rot / (2.0 * sigma_x * sigma_x)
                    + y_rot * y_rot / (2.0 * sigma_y * sigma_y);
                (-exponent).exp()
            }
            Self::Moffat { alpha, beta } => {
                let alpha = f64::from(alpha);
                (1.0 + (dx * dx + dy * dy) / (alpha * alpha)).powf(-f64::from(beta))
            }
        }
    }

    /// The quadrature its pixel means take, by [`PixelQuadrature::sufficient_order`] at
    /// [`PIXEL_MEAN_TOLERANCE`]; `None` for an axis-aligned Gaussian, whose means separate into
    /// two closed forms.
    fn pixel_rule(self) -> Option<PixelQuadrature> {
        let analytic = match self {
            Self::Gaussian { angle: 0.0, .. } => return None,
            Self::Gaussian {
                sigma_x,
                sigma_y,
                angle,
            } => {
                // The inverse covariance's diagonal: the curvature along each pixel axis at a fixed
                // offset along the other.
                let (sin_a, cos_a) = f64::from(angle).sin_cos();
                let (wide, narrow) = (
                    1.0 / f64::from(sigma_x).powi(2),
                    1.0 / f64::from(sigma_y).powi(2),
                );
                let a = cos_a * cos_a * wide + sin_a * sin_a * narrow;
                let c = sin_a * sin_a * wide + cos_a * cos_a * narrow;
                AnalyticProfile::Gaussian {
                    sigma: 1.0 / a.max(c).sqrt(),
                }
            }
            Self::Moffat { alpha, beta } => AnalyticProfile::Moffat {
                alpha: f64::from(alpha),
                beta: f64::from(beta),
            },
        };
        let order = PixelQuadrature::sufficient_order(analytic, PIXEL_MEAN_TOLERANCE)
            .expect("a fixture's profile is wide enough for a 16-point rule");
        Some(PixelQuadrature::gauss_legendre(order))
    }

    /// Radius, in pixels, past which the profile contributes negligibly.
    ///
    /// The Gaussian forms cut at 4σ, where the profile is down to `exp(-8)` ≈ 3.4e-4 of peak.
    /// Moffat's power-law wings decay far slower than an exponential, so it needs 8α to reach a
    /// comparable floor.
    pub(crate) fn radius(self) -> i32 {
        match self {
            Self::Gaussian {
                sigma_x, sigma_y, ..
            } => (4.0 * sigma_x.max(sigma_y)).ceil() as i32,
            Self::Moffat { alpha, .. } => (8.0 * alpha).ceil() as i32,
        }
    }
}

/// One star to render into a fixture: where it sits, how bright its peak is, and its shape.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SyntheticStar {
    /// Centre, in pixel coordinates. Sub-pixel positions are meaningful.
    pub(crate) center: Vec2,
    /// The profile's peak above the background, at its centre point; a pixel holds the profile's
    /// mean over its area, which is lower.
    pub(crate) amplitude: f32,
    pub(crate) profile: StarProfile,
}

impl SyntheticStar {
    pub(crate) const fn new(center: Vec2, amplitude: f32, profile: StarProfile) -> Self {
        Self {
            center,
            amplitude,
            profile,
        }
    }

    /// Radius past which this star contributes negligibly.
    pub(crate) fn radius(self) -> i32 {
        self.profile.radius()
    }

    /// The profile at the point `(x, y)`.
    pub(crate) fn value_at(self, x: f32, y: f32) -> f32 {
        let (dx, dy) = (x - self.center.x, y - self.center.y);
        (f64::from(self.amplitude) * self.profile.shape_at(f64::from(dx), f64::from(dy))) as f32
    }

    /// This star as its pixels record it.
    pub(crate) fn pixels(self) -> StarPixels {
        StarPixels {
            star: self,
            rule: self.profile.pixel_rule(),
        }
    }

    /// Add into `pixels`, visiting only the pixels within [`Self::radius`].
    #[expect(
        clippy::cast_sign_loss,
        reason = "synthetic fixtures are small images with non-negative coordinates"
    )]
    pub(crate) fn add_to(self, pixels: &mut Buffer2<f32>) {
        let (width, height) = (pixels.width(), pixels.height());
        let radius = self.radius();
        let cx = self.center.x.round() as i32;
        let cy = self.center.y.round() as i32;

        let x_min = (cx - radius).max(0) as usize;
        let x_max = ((cx + radius).max(0) as usize).min(width - 1);
        let y_min = (cy - radius).max(0) as usize;
        let y_max = ((cy + radius).max(0) as usize).min(height - 1);

        let star = self.pixels();
        for py in y_min..=y_max {
            let row = &mut pixels.row_mut(py)[x_min..=x_max];
            for (offset, sample) in row.iter_mut().enumerate() {
                *sample += star.value(x_min + offset, py);
            }
        }
    }

    /// Add into `pixels`, visiting every pixel — no truncation edge.
    pub(crate) fn add_exact(self, pixels: &mut Buffer2<f32>) {
        let height = pixels.height();
        let star = self.pixels();
        for py in 0..height {
            for (px, sample) in pixels.row_mut(py).iter_mut().enumerate() {
                *sample += star.value(px, py);
            }
        }
    }

    /// A `size` buffer on a flat `bg` holding exactly this star, rendered untruncated.
    pub(crate) fn stamp(self, size: Size2us, bg: f32) -> Buffer2<f32> {
        let mut pixels = Buffer2::new_filled(size, bg);
        self.add_exact(&mut pixels);
        pixels
    }
}

/// A star with the quadrature its pixel means take, chosen once for all of its pixels.
#[derive(Debug, Clone, Copy)]
pub(crate) struct StarPixels {
    star: SyntheticStar,
    rule: Option<PixelQuadrature>,
}

impl StarPixels {
    /// What the star adds to the pixel `(x, y)`: the amplitude times the profile's mean over it.
    pub(crate) fn value(&self, x: usize, y: usize) -> f32 {
        let dx = x as f64 - f64::from(self.star.center.x);
        let dy = y as f64 - f64::from(self.star.center.y);
        let mean = match (self.rule, self.star.profile) {
            (Some(rule), profile) => rule.integrate(dx, dy, |x, y| profile.shape_at(x, y)),
            (
                None,
                StarProfile::Gaussian {
                    sigma_x, sigma_y, ..
                },
            ) => {
                let along = |sigma: f32, d: f64| {
                    PixelGaussian {
                        sigma: f64::from(sigma),
                    }
                    .mean_at(d)
                };
                along(sigma_x, dx) * along(sigma_y, dy)
            }
            (None, StarProfile::Moffat { .. }) => unreachable!("a Moffat's means take a rule"),
        };
        (f64::from(self.star.amplitude) * mean) as f32
    }
}

#[cfg(test)]
mod tests;
