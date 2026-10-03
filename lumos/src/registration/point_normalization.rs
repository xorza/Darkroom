//! [`PointNormalization`]: the change of coordinates that conditions a fit.

use std::f64::consts::SQRT_2;

use glam::DVec2;

use crate::registration::transform::Transform;

/// The change of coordinates that conditions a fit: subtract `center`, divide by `scale`, so the
/// fitted points land in a box of order 1 around the origin.
///
/// Every estimator and distortion model needs it for one reason: its design matrix is built from
/// products or powers of the input coordinates, and raw pixel magnitudes (thousands) multiplied
/// together swamp the terms of order one. They pick `center` and `scale` by different rules
/// ([`Self::hartley`], [`Self::around`], or their own), so only the mapping lives here.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PointNormalization {
    center: DVec2,
    scale: f64,
}

impl PointNormalization {
    const IDENTITY: Self = Self {
        center: DVec2::ZERO,
        scale: 1.0,
    };

    pub(crate) fn new(center: DVec2, scale: f64) -> Self {
        debug_assert!(
            scale.is_finite() && scale > 0.0,
            "a normalization scale is finite and positive, got {scale}"
        );
        Self { center, scale }
    }

    /// Hartley's normalization: the centroid as center, and the scale that puts the average
    /// distance from it at √2. No points, or points that all coincide, give the identity: there is
    /// no spread to condition.
    pub(crate) fn hartley(points: &[DVec2]) -> Self {
        if points.is_empty() {
            return Self::IDENTITY;
        }
        let center = centroid(points);
        match average_distance(points, center) {
            Some(distance) => Self::new(center, distance / SQRT_2),
            None => Self::IDENTITY,
        }
    }

    /// `center` as given, and the average distance of `points` from it as scale; a unit scale when
    /// every point sits on `center`.
    pub(crate) fn around(points: &[DVec2], center: DVec2) -> Self {
        Self::new(center, average_distance(points, center).unwrap_or(1.0))
    }

    #[inline]
    pub(crate) fn normalize(self, p: DVec2) -> DVec2 {
        (p - self.center) / self.scale
    }

    /// Map a point from normalized space back to pixel space.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "only TPS denormalizes whole points, and TPS has no caller outside its own \
                      tests until the registration pipeline takes it up; see `tps/mod.rs`"
        )
    )]
    #[inline]
    pub(crate) fn denormalize(self, p: DVec2) -> DVec2 {
        p * self.scale + self.center
    }

    /// Map a displacement into normalized space. A displacement is a difference of two points, so
    /// `center` cancels and only `scale` acts.
    #[inline]
    pub(crate) fn normalize_delta(self, d: DVec2) -> DVec2 {
        d / self.scale
    }

    #[inline]
    pub(crate) fn denormalize_delta(self, d: DVec2) -> DVec2 {
        d * self.scale
    }

    /// [`Self::normalize`] as a transform.
    pub(crate) fn normalizing_transform(self) -> Transform {
        let inverse = 1.0 / self.scale;
        Transform::affine([
            inverse,
            0.0,
            -self.center.x * inverse,
            0.0,
            inverse,
            -self.center.y * inverse,
        ])
    }

    /// [`Self::denormalize`] as a transform, built directly rather than by inverting
    /// [`Self::normalizing_transform`].
    pub(crate) const fn denormalizing_transform(self) -> Transform {
        Transform::affine([
            self.scale,
            0.0,
            self.center.x,
            0.0,
            self.scale,
            self.center.y,
        ])
    }
}

/// The mean of `points`; the origin for none.
pub(crate) fn centroid(points: &[DVec2]) -> DVec2 {
    if points.is_empty() {
        return DVec2::ZERO;
    }
    points.iter().sum::<DVec2>() / points.len() as f64
}

/// The mean distance of `points` from `center`, or `None` when it is zero: no spread to scale by.
fn average_distance(points: &[DVec2], center: DVec2) -> Option<f64> {
    let total: f64 = points.iter().map(|p| (*p - center).length()).sum();
    let average = total / points.len() as f64;
    (average > 0.0).then_some(average)
}

#[cfg(test)]
mod tests {
    use std::f64::consts::SQRT_2;

    use glam::DVec2;

    use crate::registration::point_normalization::{PointNormalization, centroid};

    #[test]
    fn centroid_is_the_mean_and_the_origin_for_none() {
        assert_eq!(centroid(&[]), DVec2::ZERO);
        assert_eq!(centroid(&[DVec2::new(7.0, -3.0)]), DVec2::new(7.0, -3.0));
        // ((1 + 3 + 5) / 3, (2 + 4 + 6) / 3) = (3, 4).
        let points = [
            DVec2::new(1.0, 2.0),
            DVec2::new(3.0, 4.0),
            DVec2::new(5.0, 6.0),
        ];
        assert_eq!(centroid(&points), DVec2::new(3.0, 4.0));
    }

    /// The square (0,0)–(10,10): centroid (5, 5), every corner 5√2 away, so the Hartley scale is
    /// 5√2 / √2 = 5 and the corners normalize to (±1, ±1), average distance √2. Both transforms
    /// agree with the point maps, and denormalizing undoes normalizing.
    #[test]
    fn hartley_puts_the_average_distance_at_root_two() {
        let points = [
            DVec2::new(0.0, 0.0),
            DVec2::new(10.0, 0.0),
            DVec2::new(10.0, 10.0),
            DVec2::new(0.0, 10.0),
        ];
        let normalization = PointNormalization::hartley(&points);
        assert_eq!(
            normalization,
            PointNormalization::new(DVec2::new(5.0, 5.0), 5.0)
        );
        let expected = [
            DVec2::new(-1.0, -1.0),
            DVec2::new(1.0, -1.0),
            DVec2::new(1.0, 1.0),
            DVec2::new(-1.0, 1.0),
        ];
        for (&point, &normalized) in points.iter().zip(&expected) {
            assert_eq!(normalization.normalize(point), normalized);
            assert_eq!(
                normalization.normalizing_transform().apply(point),
                normalized
            );
            assert_eq!(
                normalization.denormalizing_transform().apply(normalized),
                point
            );
            assert_eq!(normalization.denormalize(normalized), point);
        }
        let average = expected.iter().map(|p| p.length()).sum::<f64>() / expected.len() as f64;
        assert_eq!(average, SQRT_2);
    }

    #[test]
    fn no_spread_normalizes_to_the_identity() {
        assert_eq!(
            PointNormalization::hartley(&[]),
            PointNormalization::new(DVec2::ZERO, 1.0)
        );
        let coincident = [DVec2::new(5.0, 5.0); 4];
        assert_eq!(
            PointNormalization::hartley(&coincident),
            PointNormalization::new(DVec2::ZERO, 1.0)
        );
        assert_eq!(
            PointNormalization::around(&coincident, DVec2::new(5.0, 5.0)),
            PointNormalization::new(DVec2::new(5.0, 5.0), 1.0)
        );
    }

    /// Unit squares at 10¹⁰: the centroid is 10¹⁰ + ½ and every corner is √2/2 from it, so the
    /// corners normalize to (±1, ±1) exactly — the offset is removed before any scaling.
    #[test]
    fn hartley_conditions_points_far_from_the_origin() {
        let base = 1e10;
        let points = [
            DVec2::new(base, base),
            DVec2::new(base + 1.0, base),
            DVec2::new(base, base + 1.0),
            DVec2::new(base + 1.0, base + 1.0),
        ];
        let normalization = PointNormalization::hartley(&points);
        assert_eq!(normalization.normalize(points[3]), DVec2::new(1.0, 1.0));
        assert_eq!(
            centroid(&points.map(|p| normalization.normalize(p))),
            DVec2::ZERO
        );
    }

    /// `around` keeps the given center and scales by the mean distance from it: the points (3, 4)
    /// and (−3, −4) are both 5 from the origin.
    #[test]
    fn around_scales_by_the_mean_distance_from_its_center() {
        let points = [DVec2::new(3.0, 4.0), DVec2::new(-3.0, -4.0)];
        let normalization = PointNormalization::around(&points, DVec2::ZERO);
        assert_eq!(normalization, PointNormalization::new(DVec2::ZERO, 5.0));
        assert_eq!(
            normalization.normalize_delta(DVec2::new(10.0, 0.0)),
            DVec2::new(2.0, 0.0)
        );
        assert_eq!(
            normalization.denormalize_delta(DVec2::new(2.0, 0.0)),
            DVec2::new(10.0, 0.0)
        );
    }
}
