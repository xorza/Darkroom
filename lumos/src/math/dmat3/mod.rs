//! Row-major 3x3 matrix of f64 values.

use glam::DVec2;
use std::ops::{Index, IndexMut, Mul};

/// Row-major 3x3 matrix of f64 values.
///
/// Memory layout:
/// ```text
/// | m[0] m[1] m[2] |
/// | m[3] m[4] m[5] |
/// | m[6] m[7] m[8] |
/// ```
///
/// For 2D homogeneous transforms this maps to:
/// ```text
/// | a  b  tx |
/// | c  d  ty |
/// | g  h  1  |
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct DMat3 {
    data: [f64; 9],
}

impl DMat3 {
    /// Create from a raw array in row-major order.
    #[inline]
    pub(crate) const fn from_array(data: [f64; 9]) -> Self {
        Self { data }
    }

    /// Create the 3x3 identity matrix.
    #[inline]
    pub(crate) const fn identity() -> Self {
        Self {
            data: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        }
    }

    /// Reference to the underlying row-major array.
    #[inline]
    pub(crate) const fn as_array(&self) -> &[f64; 9] {
        &self.data
    }

    /// Matrix multiplication: `self * rhs`.
    #[inline]
    pub(crate) fn mul_mat(&self, rhs: &DMat3) -> DMat3 {
        let a = &self.data;
        let b = &rhs.data;
        DMat3 {
            data: [
                a[0] * b[0] + a[1] * b[3] + a[2] * b[6],
                a[0] * b[1] + a[1] * b[4] + a[2] * b[7],
                a[0] * b[2] + a[1] * b[5] + a[2] * b[8],
                a[3] * b[0] + a[4] * b[3] + a[5] * b[6],
                a[3] * b[1] + a[4] * b[4] + a[5] * b[7],
                a[3] * b[2] + a[4] * b[5] + a[5] * b[8],
                a[6] * b[0] + a[7] * b[3] + a[8] * b[6],
                a[6] * b[1] + a[7] * b[4] + a[8] * b[7],
                a[6] * b[2] + a[7] * b[5] + a[8] * b[8],
            ],
        }
    }

    /// Compute the determinant.
    #[inline]
    pub(crate) fn determinant(&self) -> f64 {
        let d = &self.data;
        d[0] * (d[4] * d[8] - d[5] * d[7]) - d[1] * (d[3] * d[8] - d[5] * d[6])
            + d[2] * (d[3] * d[7] - d[4] * d[6])
    }

    /// Compute the matrix inverse, or `None` if singular.
    ///
    /// The singularity threshold scales **down** with sub-unit element magnitude (`det` scales
    /// with magnitude cubed, so a fixed epsilon would call a well-conditioned `1e-5·I`,
    /// det = 1e-15, singular) but is capped at the absolute `1e-12` above unit scale: 2D
    /// homogeneous transforms legitimately carry translations of ~1e4–1e10 px that inflate the
    /// element magnitude without inflating the determinant.
    pub(crate) fn inverse(&self) -> Option<DMat3> {
        let det = self.determinant();
        let scale = self.data.iter().fold(0.0f64, |m, v| m.max(v.abs()));
        if det.abs() <= 1e-12 * scale.powi(3).min(1.0) {
            return None;
        }
        let inv_det = 1.0 / det;
        let d = &self.data;
        Some(DMat3 {
            data: [
                (d[4] * d[8] - d[5] * d[7]) * inv_det,
                (d[2] * d[7] - d[1] * d[8]) * inv_det,
                (d[1] * d[5] - d[2] * d[4]) * inv_det,
                (d[5] * d[6] - d[3] * d[8]) * inv_det,
                (d[0] * d[8] - d[2] * d[6]) * inv_det,
                (d[2] * d[3] - d[0] * d[5]) * inv_det,
                (d[3] * d[7] - d[4] * d[6]) * inv_det,
                (d[1] * d[6] - d[0] * d[7]) * inv_det,
                (d[0] * d[4] - d[1] * d[3]) * inv_det,
            ],
        })
    }

    /// Apply this matrix as a 2D homogeneous transform to a point.
    ///
    /// Computes `(x', y')` where:
    /// ```text
    /// w  = m[6]*x + m[7]*y + m[8]
    /// x' = (m[0]*x + m[1]*y + m[2]) / w
    /// y' = (m[3]*x + m[4]*y + m[5]) / w
    /// ```
    ///
    /// A point on a homography's horizon (`w ≈ 0`) maps to infinity rather than
    /// panicking, so a warp treats it as out-of-bounds (→ border) instead of
    /// crashing. `INFINITY` (not `NaN`) is deliberate: `inf as i32` saturates to
    /// `i32::MAX` and fails the sampler's bounds check, whereas `NaN as i32` is 0
    /// and would read pixel (0,0). Affine/similarity/euclidean have a `[0,0,1]`
    /// bottom row, so `w` is structurally 1 and this branch never fires for them.
    #[inline]
    pub(crate) fn transform_point(&self, p: DVec2) -> DVec2 {
        let d = &self.data;
        let w = d[6] * p.x + d[7] * p.y + d[8];
        if w.abs() <= f64::EPSILON {
            return DVec2::splat(f64::INFINITY);
        }
        let x_prime = (d[0] * p.x + d[1] * p.y + d[2]) / w;
        let y_prime = (d[3] * p.x + d[4] * p.y + d[5]) / w;
        DVec2::new(x_prime, y_prime)
    }
}

impl Default for DMat3 {
    #[inline]
    fn default() -> Self {
        Self::identity()
    }
}

impl From<[f64; 9]> for DMat3 {
    #[inline]
    fn from(data: [f64; 9]) -> Self {
        Self { data }
    }
}

impl From<DMat3> for [f64; 9] {
    #[inline]
    fn from(m: DMat3) -> Self {
        m.data
    }
}

impl Index<usize> for DMat3 {
    type Output = f64;
    #[inline]
    fn index(&self, idx: usize) -> &f64 {
        &self.data[idx]
    }
}

impl IndexMut<usize> for DMat3 {
    #[inline]
    fn index_mut(&mut self, idx: usize) -> &mut f64 {
        &mut self.data[idx]
    }
}

impl Mul for DMat3 {
    type Output = DMat3;
    #[inline]
    fn mul(self, rhs: DMat3) -> DMat3 {
        self.mul_mat(&rhs)
    }
}

impl Mul<DVec2> for DMat3 {
    type Output = DVec2;
    /// Homogeneous point transform: `matrix * point`.
    #[inline]
    fn mul(self, rhs: DVec2) -> DVec2 {
        self.transform_point(rhs)
    }
}

impl Mul<f64> for DMat3 {
    type Output = DMat3;
    /// Scalar multiplication: `matrix * scalar`.
    #[inline]
    fn mul(self, rhs: f64) -> DMat3 {
        let mut out = self;
        for v in out.data.iter_mut() {
            *v *= rhs;
        }
        out
    }
}

impl Mul<DMat3> for f64 {
    type Output = DMat3;
    /// Scalar multiplication: `scalar * matrix`.
    #[inline]
    fn mul(self, rhs: DMat3) -> DMat3 {
        rhs * self
    }
}

/// Construction and element access used only by tests.
#[cfg(test)]
impl DMat3 {
    /// Create from three row arrays.
    pub(crate) const fn from_rows(row0: [f64; 3], row1: [f64; 3], row2: [f64; 3]) -> Self {
        Self {
            data: [
                row0[0], row0[1], row0[2], row1[0], row1[1], row1[2], row2[0], row2[1], row2[2],
            ],
        }
    }

    /// Consume and return the underlying array.
    pub(crate) const fn to_array(self) -> [f64; 9] {
        self.data
    }

    /// Mutable element access, to perturb individual entries.
    pub(crate) fn as_array_mut(&mut self) -> &mut [f64; 9] {
        &mut self.data
    }

    /// Frobenius norm of the difference from the identity matrix. Test-only diagnostic.
    pub(crate) fn deviation_from_identity(&self) -> f64 {
        let d = &self.data;
        let d0 = d[0] - 1.0;
        let d4 = d[4] - 1.0;
        let d8 = d[8] - 1.0;
        (d0 * d0
            + d[1] * d[1]
            + d[2] * d[2]
            + d[3] * d[3]
            + d4 * d4
            + d[5] * d[5]
            + d[6] * d[6]
            + d[7] * d[7]
            + d8 * d8)
            .sqrt()
    }
}

#[cfg(test)]
mod tests;
