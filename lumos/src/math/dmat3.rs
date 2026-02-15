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
pub struct DMat3 {
    data: [f64; 9],
}

impl DMat3 {
    /// Create from a raw array in row-major order.
    #[inline]
    pub const fn from_array(data: [f64; 9]) -> Self {
        Self { data }
    }

    /// Create the 3x3 identity matrix.
    #[inline]
    pub const fn identity() -> Self {
        Self {
            data: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        }
    }

    /// Create from three row arrays.
    #[inline]
    pub const fn from_rows(row0: [f64; 3], row1: [f64; 3], row2: [f64; 3]) -> Self {
        Self {
            data: [
                row0[0], row0[1], row0[2], row1[0], row1[1], row1[2], row2[0], row2[1], row2[2],
            ],
        }
    }

    /// Reference to the underlying row-major array.
    #[inline]
    pub const fn as_array(&self) -> &[f64; 9] {
        &self.data
    }

    /// Mutable reference to the underlying row-major array.
    #[inline]
    pub fn as_array_mut(&mut self) -> &mut [f64; 9] {
        &mut self.data
    }

    /// Consume and return the underlying array.
    #[inline]
    pub const fn to_array(self) -> [f64; 9] {
        self.data
    }

    /// Matrix multiplication: `self * rhs`.
    #[inline]
    pub fn mul_mat(&self, rhs: &DMat3) -> DMat3 {
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
    pub fn determinant(&self) -> f64 {
        let d = &self.data;
        d[0] * (d[4] * d[8] - d[5] * d[7]) - d[1] * (d[3] * d[8] - d[5] * d[6])
            + d[2] * (d[3] * d[7] - d[4] * d[6])
    }

    /// Compute the matrix inverse, or `None` if singular.
    ///
    /// Uses a fixed threshold of 1e-12 for singularity detection,
    /// appropriate for pixel-scale coordinates (typical values 0-10000).
    pub fn inverse(&self) -> Option<DMat3> {
        let det = self.determinant();
        if det.abs() < 1e-12 {
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
    /// # Panics (debug)
    /// Panics if `w` is near zero (point at infinity).
    #[inline]
    pub fn transform_point(&self, p: DVec2) -> DVec2 {
        let d = &self.data;
        let w = d[6] * p.x + d[7] * p.y + d[8];
        assert!(
            w.abs() > f64::EPSILON,
            "transform_point: w ≈ 0 (point at infinity)"
        );
        let x_prime = (d[0] * p.x + d[1] * p.y + d[2]) / w;
        let y_prime = (d[3] * p.x + d[4] * p.y + d[5]) / w;
        DVec2::new(x_prime, y_prime)
    }

    /// Frobenius norm of the difference from the identity matrix.
    pub fn deviation_from_identity(&self) -> f64 {
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

// ---------------------------------------------------------------------------
// Trait implementations
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f64 = 1e-10;

    fn approx_eq(a: f64, b: f64) -> bool {
        (a - b).abs() < EPS
    }

    fn mat_approx_eq(a: &DMat3, b: &DMat3) -> bool {
        a.as_array()
            .iter()
            .zip(b.as_array().iter())
            .all(|(x, y)| approx_eq(*x, *y))
    }

    // -- Construction ---------------------------------------------------------

    #[test]
    fn test_from_array() {
        let data = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0];
        let m = DMat3::from_array(data);
        assert_eq!(*m.as_array(), data);
    }

    #[test]
    fn test_identity() {
        let m = DMat3::identity();
        assert_eq!(*m.as_array(), [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn test_from_rows() {
        let m = DMat3::from_rows([1.0, 2.0, 3.0], [4.0, 5.0, 6.0], [7.0, 8.0, 9.0]);
        assert_eq!(*m.as_array(), [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0]);
    }

    #[test]
    fn test_default_is_identity() {
        assert_eq!(DMat3::default(), DMat3::identity());
    }

    // -- Access ---------------------------------------------------------------

    #[test]
    fn test_index() {
        let m = DMat3::from_array([10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0]);
        assert!(approx_eq(m[0], 10.0));
        assert!(approx_eq(m[4], 50.0));
        assert!(approx_eq(m[8], 90.0));
    }

    #[test]
    fn test_index_mut() {
        let mut m = DMat3::identity();
        m[2] = 5.0;
        m[5] = -3.0;
        assert!(approx_eq(m[2], 5.0));
        assert!(approx_eq(m[5], -3.0));
    }

    #[test]
    fn test_as_array_mut() {
        let mut m = DMat3::identity();
        let arr = m.as_array_mut();
        arr[2] = 7.0;
        assert!(approx_eq(m[2], 7.0));
    }

    #[test]
    fn test_to_array() {
        let data = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0];
        let m = DMat3::from_array(data);
        assert_eq!(m.to_array(), data);
    }

    // -- Conversions ----------------------------------------------------------

    #[test]
    fn test_from_array_trait() {
        let data = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
        let m: DMat3 = data.into();
        assert_eq!(m, DMat3::identity());
    }

    #[test]
    fn test_into_array() {
        let m = DMat3::identity();
        let arr: [f64; 9] = m.into();
        assert_eq!(arr, [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);
    }

    // -- Determinant ----------------------------------------------------------

    #[test]
    fn test_determinant_identity() {
        assert!(approx_eq(DMat3::identity().determinant(), 1.0));
    }

    #[test]
    fn test_determinant_singular() {
        // Two identical rows → det = 0
        let m = DMat3::from_rows([1.0, 2.0, 3.0], [1.0, 2.0, 3.0], [4.0, 5.0, 6.0]);
        assert!(approx_eq(m.determinant(), 0.0));
    }

    #[test]
    fn test_determinant_known() {
        let m = DMat3::from_rows([2.0, 0.0, 0.0], [0.0, 3.0, 0.0], [0.0, 0.0, 4.0]);
        assert!(approx_eq(m.determinant(), 24.0));
    }

    #[test]
    fn test_determinant_negative() {
        // Swapping two rows negates the determinant
        let m = DMat3::from_rows([0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
        assert!(approx_eq(m.determinant(), -1.0));
    }

    // -- Inverse --------------------------------------------------------------

    #[test]
    fn test_inverse_identity() {
        let inv = DMat3::identity().inverse().unwrap();
        assert!(mat_approx_eq(&inv, &DMat3::identity()));
    }

    #[test]
    fn test_inverse_singular_returns_none() {
        let m = DMat3::from_array([0.0; 9]);
        assert!(m.inverse().is_none());
    }

    #[test]
    fn test_inverse_roundtrip() {
        let m = DMat3::from_rows([1.0, 2.0, 3.0], [0.0, 1.0, 4.0], [5.0, 6.0, 0.0]);
        let inv = m.inverse().unwrap();
        let product = m.mul_mat(&inv);
        assert!(
            mat_approx_eq(&product, &DMat3::identity()),
            "M * M^-1 should be identity, got {:?}",
            product
        );
    }

    #[test]
    fn test_inverse_diagonal() {
        let m = DMat3::from_rows([2.0, 0.0, 0.0], [0.0, 4.0, 0.0], [0.0, 0.0, 5.0]);
        let inv = m.inverse().unwrap();
        let expected = DMat3::from_rows([0.5, 0.0, 0.0], [0.0, 0.25, 0.0], [0.0, 0.0, 0.2]);
        assert!(mat_approx_eq(&inv, &expected));
    }

    // -- Multiplication -------------------------------------------------------

    #[test]
    fn test_mul_identity() {
        let m = DMat3::from_rows([1.0, 2.0, 3.0], [4.0, 5.0, 6.0], [7.0, 8.0, 9.0]);
        let product = m.mul_mat(&DMat3::identity());
        assert!(mat_approx_eq(&product, &m));

        let product2 = DMat3::identity().mul_mat(&m);
        assert!(mat_approx_eq(&product2, &m));
    }

    #[test]
    fn test_mul_known() {
        let a = DMat3::from_rows([1.0, 2.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]);
        let b = DMat3::from_rows([1.0, 0.0, 3.0], [0.0, 1.0, 4.0], [0.0, 0.0, 1.0]);
        let c = a.mul_mat(&b);
        // Row 0: [1*1+2*0+0*0, 1*0+2*1+0*0, 1*3+2*4+0*1] = [1, 2, 11]
        // Row 1: [0, 1, 4]
        // Row 2: [0, 0, 1]
        let expected = DMat3::from_rows([1.0, 2.0, 11.0], [0.0, 1.0, 4.0], [0.0, 0.0, 1.0]);
        assert!(mat_approx_eq(&c, &expected));
    }

    #[test]
    fn test_mul_operator() {
        let a = DMat3::from_rows([2.0, 0.0, 0.0], [0.0, 3.0, 0.0], [0.0, 0.0, 1.0]);
        let b = DMat3::from_rows([1.0, 0.0, 5.0], [0.0, 1.0, 7.0], [0.0, 0.0, 1.0]);
        let c = a * b;
        let expected = DMat3::from_rows([2.0, 0.0, 10.0], [0.0, 3.0, 21.0], [0.0, 0.0, 1.0]);
        assert!(mat_approx_eq(&c, &expected));
    }

    #[test]
    fn test_mul_non_commutative() {
        let a = DMat3::from_rows([1.0, 2.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]);
        let b = DMat3::from_rows([1.0, 0.0, 0.0], [3.0, 1.0, 0.0], [0.0, 0.0, 1.0]);
        let ab = a * b;
        let ba = b * a;
        // A*B != B*A for these matrices
        assert!(!mat_approx_eq(&ab, &ba));
    }

    // -- transform_point ------------------------------------------------------

    #[test]
    fn test_transform_point_identity() {
        let m = DMat3::identity();
        let p = m.transform_point(DVec2::new(5.0, 7.0));
        assert!(approx_eq(p.x, 5.0));
        assert!(approx_eq(p.y, 7.0));
    }

    #[test]
    fn test_transform_point_translation() {
        let m = DMat3::from_array([1.0, 0.0, 10.0, 0.0, 1.0, -5.0, 0.0, 0.0, 1.0]);
        let p = m.transform_point(DVec2::new(3.0, 4.0));
        assert!(approx_eq(p.x, 13.0));
        assert!(approx_eq(p.y, -1.0));
    }

    #[test]
    fn test_transform_point_perspective() {
        let m = DMat3::from_array([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.001, 0.0, 1.0]);
        let p = m.transform_point(DVec2::new(100.0, 0.0));
        // w = 0.001 * 100 + 1 = 1.1
        assert!((p.x - 90.909).abs() < 0.01);
        assert!(approx_eq(p.y, 0.0));
    }

    #[test]
    fn test_transform_point_roundtrip() {
        let m = DMat3::from_rows([1.1, 0.2, 5.0], [-0.1, 0.9, -3.0], [0.0, 0.0, 1.0]);
        let inv = m.inverse().unwrap();
        let p = DVec2::new(10.0, -5.0);
        let p2 = inv.transform_point(m.transform_point(p));
        assert!(approx_eq(p2.x, p.x));
        assert!(approx_eq(p2.y, p.y));
    }

    // -- deviation_from_identity ----------------------------------------------

    #[test]
    fn test_deviation_from_identity_zero() {
        assert!(approx_eq(DMat3::identity().deviation_from_identity(), 0.0));
    }

    #[test]
    fn test_deviation_from_identity_nonzero() {
        let m = DMat3::from_array([1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);
        // Only m[2] differs by 1.0
        assert!(approx_eq(m.deviation_from_identity(), 1.0));
    }

    #[test]
    fn test_deviation_from_identity_multiple_elements() {
        // Diagonal elements differ by 1.0 each: (2-1)^2 + (2-1)^2 + (2-1)^2 = 3
        let m = DMat3::from_rows([2.0, 0.0, 0.0], [0.0, 2.0, 0.0], [0.0, 0.0, 2.0]);
        assert!(approx_eq(m.deviation_from_identity(), 3.0_f64.sqrt()));
    }

    // -- Index bounds ---------------------------------------------------------

    #[test]
    #[should_panic]
    fn test_index_out_of_bounds() {
        let m = DMat3::identity();
        let _ = m[9];
    }

    // -- Mul<f64> / f64 * DMat3 -----------------------------------------------

    #[test]
    fn test_mul_scalar() {
        let m = DMat3::from_array([1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0]);
        let scaled = m * 2.0;
        assert_eq!(
            scaled.to_array(),
            [2.0, 4.0, 6.0, 8.0, 10.0, 12.0, 14.0, 16.0, 18.0]
        );
    }

    #[test]
    fn test_scalar_mul_commutative() {
        let m = DMat3::from_array([1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0]);
        let a = m * 3.0;
        let b = 3.0 * m;
        assert!(mat_approx_eq(&a, &b));
    }

    #[test]
    fn test_mul_scalar_zero() {
        let m = DMat3::from_array([1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0]);
        let z = m * 0.0;
        assert_eq!(z.to_array(), [0.0; 9]);
    }

    #[test]
    fn test_mul_scalar_one_is_identity_op() {
        let m = DMat3::from_array([1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0]);
        let same = m * 1.0;
        assert!(mat_approx_eq(&same, &m));
    }
}
