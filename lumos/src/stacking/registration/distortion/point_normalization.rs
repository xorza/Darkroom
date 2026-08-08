//! Coordinate normalization shared by the distortion models.

use glam::DVec2;

/// The change of coordinates that conditions a distortion fit: subtract `center`, divide by
/// `scale`, so the fitted points land in a box of order 1 around the origin.
///
/// Both models need it for the same reason — their design matrices are built from powers or
/// radial kernels of the input coordinates, and raw pixel magnitudes (thousands) raised to
/// order 5 or fed through `r² ln r` swamp the affine terms. They pick `center` and `scale` by
/// different rules, so the fitting stays with each owner and only the mapping lives here.
#[derive(Debug, Clone, Copy)]
pub(super) struct PointNormalization {
    pub(super) center: DVec2,
    pub(super) scale: f64,
}

impl PointNormalization {
    pub(super) fn new(center: DVec2, scale: f64) -> Self {
        Self { center, scale }
    }

    /// Map a point from pixel space into normalized space.
    #[inline]
    pub(super) fn normalize(self, p: DVec2) -> DVec2 {
        (p - self.center) / self.scale
    }

    /// Map a point from normalized space back to pixel space.
    // Only TPS denormalizes whole points, and TPS has no caller until it is integrated into the
    // registration pipeline — see the module note in `tps/mod.rs`.
    #[allow(dead_code)]
    #[inline]
    pub(super) fn denormalize(self, p: DVec2) -> DVec2 {
        p * self.scale + self.center
    }

    /// Map a displacement from pixel space into normalized space.
    ///
    /// A displacement is a difference of two points, so `center` cancels and only `scale` acts.
    #[inline]
    pub(super) fn normalize_delta(self, d: DVec2) -> DVec2 {
        d / self.scale
    }

    /// Map a displacement from normalized space back to pixel space.
    #[inline]
    pub(super) fn denormalize_delta(self, d: DVec2) -> DVec2 {
        d * self.scale
    }
}
