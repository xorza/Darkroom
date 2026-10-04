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

use glam::Vec2;
use imaginarium::Buffer2;

use crate::math::pixel_gaussian::PixelGaussian;
use crate::math::pixel_quadrature::{AnalyticProfile, PixelQuadrature};
use crate::math::size2us::Size2us;

/// How closely a pixel mean without a closed form is integrated, relative to the peak: far under
/// the f32 rounding of a sample, and under any tolerance a test asserts.
const PIXEL_MEAN_TOLERANCE: f64 = 1e-12;

/// The analytic shape of a star profile, parameterized by peak amplitude.
#[derive(Debug, Clone, Copy)]
pub(crate) enum StarProfile {
    /// Circular Gaussian: `exp(-r² / 2σ²)`.
    Gaussian {
        /// Standard deviation in pixels (FWHM = 2.355σ).
        sigma: f32,
    },
    /// Elliptical Gaussian — simulates tracking error.
    Elliptical {
        /// Sigma along the profile's own x axis, before the `angle` rotation.
        sigma_x: f32,
        /// Sigma along the profile's own y axis, before the `angle` rotation.
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
    /// Profile value at `(dx, dy)` pixels from the centre, as a fraction of the peak.
    pub(crate) fn shape_at(self, dx: f64, dy: f64) -> f64 {
        match self {
            StarProfile::Gaussian { sigma } => {
                let sigma = f64::from(sigma);
                (-(dx * dx + dy * dy) / (2.0 * sigma * sigma)).exp()
            }
            StarProfile::Elliptical {
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
            StarProfile::Moffat { alpha, beta } => {
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
            StarProfile::Gaussian { .. } | StarProfile::Elliptical { angle: 0.0, .. } => {
                return None;
            }
            StarProfile::Elliptical {
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
            StarProfile::Moffat { alpha, beta } => AnalyticProfile::Moffat {
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
            StarProfile::Gaussian { sigma } => (4.0 * sigma).ceil() as i32,
            StarProfile::Elliptical {
                sigma_x, sigma_y, ..
            } => (4.0 * sigma_x.max(sigma_y)).ceil() as i32,
            StarProfile::Moffat { alpha, .. } => (8.0 * alpha).ceil() as i32,
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
    pub(crate) fn new(center: Vec2, amplitude: f32, profile: StarProfile) -> Self {
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

    /// A `size` buffer on a flat `background` holding exactly this star, rendered untruncated.
    pub(crate) fn stamp(self, size: Size2us, background: f32) -> Buffer2<f32> {
        let mut pixels = Buffer2::new_filled(size.width, size.height, background);
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
            (None, StarProfile::Gaussian { sigma }) => {
                let gaussian = PixelGaussian {
                    sigma: f64::from(sigma),
                };
                gaussian.mean_at(dx) * gaussian.mean_at(dy)
            }
            (
                None,
                StarProfile::Elliptical {
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
mod tests {
    use crate::internals::synthetic::star_profiles::*;
    use crate::math::fwhm::{fwhm_beta_to_alpha, fwhm_to_sigma};
    use std::f32::consts::FRAC_PI_2;

    const GAUSSIAN_2: StarProfile = StarProfile::Gaussian { sigma: 2.0 };

    #[test]
    fn gaussian_peaks_at_its_centre_and_vanishes_at_the_corner() {
        let mut pixels = Buffer2::new_filled(64, 64, 0.0f32);
        SyntheticStar::new(Vec2::splat(32.0), 1.0, GAUSSIAN_2).add_to(&mut pixels);

        assert!(pixels[(32, 32)] > 0.9, "peak should be near 1.0");
        assert!(pixels[(0, 0)] < 0.001, "corner should be near 0");
    }

    #[test]
    fn gaussian_value_matches_the_closed_form() {
        let star = SyntheticStar::new(Vec2::splat(10.0), 0.8, GAUSSIAN_2);
        // At 2px off-centre with sigma 2: 0.8 * exp(-4/8) = 0.8 * exp(-0.5) = 0.485225...
        let expected = 0.8 * (-0.5f32).exp();
        assert!((star.value_at(12.0, 10.0) - expected).abs() < 1e-6);
        // Radially symmetric: the same offset along y, and along the diagonal at r² = 4.
        assert!((star.value_at(10.0, 8.0) - expected).abs() < 1e-6);
        let diagonal = star.value_at(10.0 + 2.0f32.sqrt(), 10.0 + 2.0f32.sqrt());
        assert!((diagonal - expected).abs() < 1e-6);
    }

    #[test]
    fn truncated_and_exact_agree_inside_the_radius_and_differ_outside() {
        let size = Size2us::new(64, 64);
        let star = SyntheticStar::new(Vec2::splat(32.0), 1.0, GAUSSIAN_2);

        let mut truncated = Buffer2::new_filled(size.width, size.height, 0.0f32);
        star.add_to(&mut truncated);
        let exact = star.stamp(size, 0.0);

        // Inside the 4σ = 8px box the two modes are bit-identical.
        assert_eq!(truncated[(32, 32)], exact[(32, 32)]);
        assert_eq!(truncated[32 * 64 + 39], exact[(32, 39)]);
        // Outside it, truncation drops a small but non-zero wing that `add_exact` keeps.
        assert_eq!(truncated[32 * 64 + 45], 0.0);
        let wing = exact[(32, 45)];
        assert!(
            wing > 0.0 && wing < 1e-3,
            "wing at 13px should be tiny but present, got {wing}"
        );
    }

    #[test]
    fn stamp_lays_the_star_over_a_flat_background() {
        let stamp =
            SyntheticStar::new(Vec2::splat(10.0), 0.5, GAUSSIAN_2).stamp(Size2us::new(21, 21), 0.1);
        // The peak pixel holds the profile's mean over it, at σ 2 m(0) = 4·√(π/2)·erf(1/(4√2)) =
        // 0.989680 per axis: 0.1 + 0.5·m(0)² = 0.589734, to the f32 rounding of the sum.
        assert!((stamp[(10, 10)] - 0.589_733_5).abs() <= 1e-7);
        // A far corner is background plus a negligible wing.
        assert!(stamp[(0, 0)] >= 0.1 && stamp[(0, 0)] < 0.1 + 1e-4);
    }

    #[test]
    fn elliptical_is_elongated_along_its_wider_axis_and_rotates_with_angle() {
        let wide = StarProfile::Elliptical {
            sigma_x: 4.0,
            sigma_y: 2.0,
            angle: 0.0,
        };
        let star = SyntheticStar::new(Vec2::splat(32.0), 1.0, wide);
        // 6px along the wide (x) axis keeps more flux than 6px along the narrow (y) axis.
        assert!(star.value_at(38.0, 32.0) > star.value_at(32.0, 38.0));

        // Rotating by 90° swaps which direction is wide.
        let turned = SyntheticStar::new(
            Vec2::splat(32.0),
            1.0,
            StarProfile::Elliptical {
                sigma_x: 4.0,
                sigma_y: 2.0,
                angle: FRAC_PI_2,
            },
        );
        assert!(turned.value_at(32.0, 38.0) > turned.value_at(38.0, 32.0));
        // The rotation is rigid: the peak and the profile's extent are unchanged.
        assert!((turned.value_at(32.0, 38.0) - star.value_at(38.0, 32.0)).abs() < 1e-6);
    }

    /// A round Gaussian turned by any angle takes the quadrature, and the same Gaussian unturned
    /// the closed form: their pixel means agree to 1e-12 of the peak, so their f32 values agree
    /// within one rounding of each, at σ 0.6 and 2.
    #[test]
    fn the_quadrature_and_the_closed_form_agree() {
        for sigma in [0.6f32, 2.0] {
            let centre = Vec2::new(8.3, 7.6);
            let closed = SyntheticStar::new(centre, 1.0, StarProfile::Gaussian { sigma }).pixels();
            let turned = SyntheticStar::new(
                centre,
                1.0,
                StarProfile::Elliptical {
                    sigma_x: sigma,
                    sigma_y: sigma,
                    angle: 1.0,
                },
            )
            .pixels();
            for y in 0..16 {
                for x in 0..16 {
                    let (a, b) = (closed.value(x, y), turned.value(x, y));
                    assert!(
                        (a - b).abs() <= f32::EPSILON * a.max(b) + 1e-12,
                        "σ {sigma} at ({x}, {y}): {a} vs {b}"
                    );
                }
            }
        }
    }

    #[test]
    fn moffat_carries_heavier_wings_than_a_gaussian_of_equal_fwhm() {
        let beta = 2.5;
        let fwhm = 4.0;
        let moffat = SyntheticStar::new(
            Vec2::splat(32.0),
            1.0,
            StarProfile::Moffat {
                alpha: fwhm_beta_to_alpha(fwhm, beta),
                beta,
            },
        );
        let gaussian = SyntheticStar::new(
            Vec2::splat(32.0),
            1.0,
            StarProfile::Gaussian {
                sigma: fwhm_to_sigma(fwhm),
            },
        );

        // Equal FWHM means they cross at the half-maximum point...
        let half = 32.0 + fwhm / 2.0;
        assert!((moffat.value_at(half, 32.0) - 0.5).abs() < 1e-5);
        assert!((gaussian.value_at(half, 32.0) - 0.5).abs() < 1e-5);
        // ...but far out, the power law dominates the exponential.
        assert!(moffat.value_at(44.0, 32.0) > gaussian.value_at(44.0, 32.0) * 100.0);
    }

    #[test]
    fn moffat_needs_a_wider_radius_than_a_gaussian_to_reach_the_same_floor() {
        // 8α vs 4σ: for equal FWHM the Moffat box is the larger one.
        let beta = 2.5;
        let moffat = StarProfile::Moffat {
            alpha: fwhm_beta_to_alpha(4.0, beta),
            beta,
        };
        let gaussian = StarProfile::Gaussian {
            sigma: fwhm_to_sigma(4.0),
        };
        assert!(moffat.radius() > gaussian.radius());
    }
}
