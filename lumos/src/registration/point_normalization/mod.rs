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
    pub(super) fn hartley(points: &[DVec2]) -> Self {
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

    pub(crate) const fn center(self) -> DVec2 {
        self.center
    }

    pub(crate) const fn scale(self) -> f64 {
        self.scale
    }

    #[inline]
    pub(crate) fn normalize(self, p: DVec2) -> DVec2 {
        (p - self.center) / self.scale
    }

    /// Map a point from normalized space back to pixel space.
    #[inline]
    pub(super) fn denormalize(self, p: DVec2) -> DVec2 {
        p * self.scale + self.center
    }

    #[inline]
    pub(super) fn denormalize_delta(self, d: DVec2) -> DVec2 {
        d * self.scale
    }

    /// [`Self::normalize`] as a transform.
    pub(super) fn normalizing_transform(self) -> Transform {
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
    pub(super) const fn denormalizing_transform(self) -> Transform {
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
pub(crate) mod internals {
    use glam::DVec2;

    use crate::registration::point_normalization::PointNormalization;

    impl PointNormalization {
        /// Map a displacement into normalized space. A displacement is a difference of two points,
        /// so `center` cancels and only `scale` acts.
        pub(crate) fn normalize_delta(self, d: DVec2) -> DVec2 {
            d / self.scale
        }
    }
}

#[cfg(test)]
mod tests;
