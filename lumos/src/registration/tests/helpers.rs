//! Shared test helpers for synthetic registration tests.

use glam::DVec2;

use crate::registration::registration_config::RegistrationMatchingConfig;
use crate::registration::transform::Transform;
use crate::star_detection::star::Star;

/// FWHM for tight stars: `max_sigma_from_fwhm` gives `σ_max` = 0.67 px, above its 0.5 px floor.
pub(super) const FWHM_TIGHT: f32 = 1.34;
/// FWHM for typical stars: `σ_max` = 1.0 px, a recovery radius of `√χ²₀.₉₉(2)` = 3.03 px.
pub(super) const FWHM_NORMAL: f32 = 2.0;

pub(super) fn matching_config(min_stars: usize, min_matches: usize) -> RegistrationMatchingConfig {
    RegistrationMatchingConfig {
        min_stars: Some(min_stars),
        min_matches,
        ..Default::default()
    }
}

/// `stars` moved by `transform`, every other field kept.
pub(super) fn map_stars(stars: &[Star], transform: &Transform) -> Vec<Star> {
    stars
        .iter()
        .map(|star| star.with_pos(transform.apply(star.pos)))
        .collect()
}

/// The largest distance between where `found` and `truth` put a point of the bounding box of
/// `points`, over a 9×9 grid spanning it: a fit's error where it is used, corners included, rather
/// than in parameters whose units differ.
pub(super) fn max_deviation(
    found: &Transform,
    truth: &Transform,
    points: impl IntoIterator<Item = DVec2>,
) -> f64 {
    let (low, high) = points.into_iter().fold(
        (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY)),
        |(low, high), p| (low.min(p), high.max(p)),
    );
    (0..9)
        .flat_map(|j| (0..9).map(move |i| DVec2::new(f64::from(i), f64::from(j)) / 8.0))
        .map(|t| low + (high - low) * t)
        .map(|p| found.apply(p).distance(truth.apply(p)))
        .fold(0.0, f64::max)
}

/// `s·R(θ)` of `scale` and `angle_deg`, then `offset`, as a similarity about a centre.
#[derive(Debug, Clone, Copy)]
pub(super) struct FieldMotion {
    pub(super) offset: DVec2,
    pub(super) angle_deg: f64,
    pub(super) scale: f64,
}

impl FieldMotion {
    /// This similarity about `(c, c)`.
    pub(super) fn about(self, c: f64) -> Transform {
        let angle = self.angle_deg.to_radians();
        let linear = Transform::similarity(DVec2::ZERO, angle, self.scale);
        let centre = DVec2::splat(c);
        Transform::similarity(
            centre + self.offset - linear.apply(centre),
            angle,
            self.scale,
        )
    }
}
