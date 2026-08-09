//! Star profile rendering for synthetic fixtures.
//!
//! A [`StarProfile`] is the analytic shape — Gaussian, elliptical Gaussian, or Moffat — and a
//! [`SyntheticStar`] binds one to a centre and a peak amplitude. This is the crate's only
//! definition of those profiles; [`PsfModel`](crate::testing::synthetic::camera::PsfModel)
//! layers flux normalization on top of it rather than re-deriving the math.
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

use crate::math::size2us::Size2us;

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
    pub(crate) fn shape_at(self, dx: f32, dy: f32) -> f32 {
        match self {
            StarProfile::Gaussian { sigma } => (-(dx * dx + dy * dy) / (2.0 * sigma * sigma)).exp(),
            StarProfile::Elliptical {
                sigma_x,
                sigma_y,
                angle,
            } => {
                let (sin_a, cos_a) = angle.sin_cos();
                let x_rot = dx * cos_a + dy * sin_a;
                let y_rot = -dx * sin_a + dy * cos_a;
                let exponent = x_rot * x_rot / (2.0 * sigma_x * sigma_x)
                    + y_rot * y_rot / (2.0 * sigma_y * sigma_y);
                (-exponent).exp()
            }
            StarProfile::Moffat { alpha, beta } => {
                (1.0 + (dx * dx + dy * dy) / (alpha * alpha)).powf(-beta)
            }
        }
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
    /// Peak value above the background.
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

    /// Value this star contributes at absolute pixel `(x, y)`.
    pub(crate) fn value_at(self, x: f32, y: f32) -> f32 {
        self.amplitude * self.profile.shape_at(x - self.center.x, y - self.center.y)
    }

    /// Add into `pixels`, visiting only the pixels within [`Self::radius`].
    pub(crate) fn add_to(self, pixels: &mut Buffer2<f32>) {
        let (width, height) = (pixels.width(), pixels.height());
        let radius = self.radius();
        let cx = self.center.x.round() as i32;
        let cy = self.center.y.round() as i32;

        let x_min = (cx - radius).max(0) as usize;
        let x_max = ((cx + radius).max(0) as usize).min(width - 1);
        let y_min = (cy - radius).max(0) as usize;
        let y_max = ((cy + radius).max(0) as usize).min(height - 1);

        for py in y_min..=y_max {
            let row = &mut pixels.row_mut(py)[x_min..=x_max];
            for (offset, sample) in row.iter_mut().enumerate() {
                let px = x_min + offset;
                *sample += self.value_at(px as f32, py as f32);
            }
        }
    }

    /// Add into `pixels`, visiting every pixel — no truncation edge.
    pub(crate) fn add_exact(self, pixels: &mut Buffer2<f32>) {
        let height = pixels.height();
        for py in 0..height {
            for (px, sample) in pixels.row_mut(py).iter_mut().enumerate() {
                *sample += self.value_at(px as f32, py as f32);
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

/// Convert Moffat parameters to FWHM.
fn moffat_fwhm(alpha: f32, beta: f32) -> f32 {
    2.0 * alpha * (2.0f32.powf(1.0 / beta) - 1.0).sqrt()
}

/// Convert FWHM to the Moffat alpha parameter, for a given beta.
pub(crate) fn fwhm_to_moffat_alpha(fwhm: f32, beta: f32) -> f32 {
    fwhm / (2.0 * (2.0f32.powf(1.0 / beta) - 1.0).sqrt())
}

#[cfg(test)]
mod tests {
    use crate::math::fwhm::{fwhm_to_sigma, sigma_to_fwhm};
    use crate::testing::synthetic::star_profiles::*;

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
        // Peak sits exactly amplitude above the background.
        assert!((stamp[(10, 10)] - 0.6).abs() < 1e-6);
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
                angle: std::f32::consts::FRAC_PI_2,
            },
        );
        assert!(turned.value_at(32.0, 38.0) > turned.value_at(38.0, 32.0));
        // The rotation is rigid: the peak and the profile's extent are unchanged.
        assert!((turned.value_at(32.0, 38.0) - star.value_at(38.0, 32.0)).abs() < 1e-6);
    }

    #[test]
    fn moffat_carries_heavier_wings_than_a_gaussian_of_equal_fwhm() {
        let beta = 2.5;
        let fwhm = 4.0;
        let moffat = SyntheticStar::new(
            Vec2::splat(32.0),
            1.0,
            StarProfile::Moffat {
                alpha: fwhm_to_moffat_alpha(fwhm, beta),
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
            alpha: fwhm_to_moffat_alpha(4.0, beta),
            beta,
        };
        let gaussian = StarProfile::Gaussian {
            sigma: fwhm_to_sigma(4.0),
        };
        assert!(moffat.radius() > gaussian.radius());
    }

    #[test]
    fn moffat_fwhm_conversion_round_trips() {
        let beta = 2.5;
        let fwhm = 4.0;
        let alpha = fwhm_to_moffat_alpha(fwhm, beta);
        assert!((moffat_fwhm(alpha, beta) - fwhm).abs() < 0.001);
    }

    #[test]
    fn fwhm_sigma_conversion_round_trips() {
        let fwhm = 4.0;
        assert!((sigma_to_fwhm(fwhm_to_sigma(fwhm)) - fwhm).abs() < 0.001);
    }
}
