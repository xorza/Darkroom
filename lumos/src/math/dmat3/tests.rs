use crate::math::dmat3::*;
use crate::testing::prelude::*;

const EPS: f64 = 1e-10;

/// Bool rather than an assertion because two tests assert matrices are *not* equal.
fn mat_approx_eq(a: &DMat3, b: &DMat3) -> bool {
    a.as_array()
        .iter()
        .zip(b.as_array().iter())
        .all(|(x, y)| is_close(*x, *y, EPS))
}

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

#[test]
fn test_index() {
    let m = DMat3::from_array([10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0]);
    assert_close!(m[0], 10.0, EPS);
    assert_close!(m[4], 50.0, EPS);
    assert_close!(m[8], 90.0, EPS);
}

#[test]
fn test_index_mut() {
    let mut m = DMat3::identity();
    m[2] = 5.0;
    m[5] = -3.0;
    assert_close!(m[2], 5.0, EPS);
    assert_close!(m[5], -3.0, EPS);
}

#[test]
fn test_as_array_mut() {
    let mut m = DMat3::identity();
    let arr = m.as_array_mut();
    arr[2] = 7.0;
    assert_close!(m[2], 7.0, EPS);
}

#[test]
fn test_to_array() {
    let data = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0];
    let m = DMat3::from_array(data);
    assert_eq!(m.to_array(), data);
}

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

#[test]
fn test_determinant_identity() {
    assert_close!(DMat3::identity().determinant(), 1.0, EPS);
}

#[test]
fn test_determinant_singular() {
    // Two identical rows → det = 0
    let m = DMat3::from_rows([1.0, 2.0, 3.0], [1.0, 2.0, 3.0], [4.0, 5.0, 6.0]);
    assert_close!(m.determinant(), 0.0, EPS);
}

#[test]
fn test_determinant_known() {
    let m = DMat3::from_rows([2.0, 0.0, 0.0], [0.0, 3.0, 0.0], [0.0, 0.0, 4.0]);
    assert_close!(m.determinant(), 24.0, EPS);
}

#[test]
fn test_determinant_negative() {
    // Swapping two rows negates the determinant
    let m = DMat3::from_rows([0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
    assert_close!(m.determinant(), -1.0, EPS);
}

#[test]
fn test_inverse_identity() {
    let inv = DMat3::identity().inverse().unwrap();
    assert!(mat_approx_eq(&inv, &DMat3::identity()));
}

#[test]
fn test_inverse_singular_returns_none() {
    let m = DMat3::from_array([0.0; 9]);
    assert!(m.inverse().is_none());

    // Rank-deficient with large elements (det = 0 but scale³ = 1e9): still singular —
    // the relative threshold must not be fooled by magnitude.
    let m = DMat3::from_rows([1e3, 0.0, 0.0], [0.0, 1e3, 0.0], [1e3, 0.0, 0.0]);
    assert!(m.inverse().is_none());
}

#[test]
fn test_inverse_small_scale_not_misclassified_singular() {
    // 1e-5·I is perfectly conditioned but det = 1e-15 — the old fixed 1e-12 threshold
    // wrongly called it singular. Relative test: 1e-15 > 1e-12·(1e-5)³ = 1e-27 → invertible,
    // inverse = 1e5·I.
    let m = DMat3::from_rows([1e-5, 0.0, 0.0], [0.0, 1e-5, 0.0], [0.0, 0.0, 1e-5]);
    let inv = m
        .inverse()
        .expect("well-conditioned small-scale matrix must invert");
    let expected = DMat3::from_rows([1e5, 0.0, 0.0], [0.0, 1e5, 0.0], [0.0, 0.0, 1e5]);
    assert!(mat_approx_eq(&inv, &expected));
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

#[test]
fn test_transform_point_identity() {
    let m = DMat3::identity();
    let p = m.transform_point(DVec2::new(5.0, 7.0));
    assert_close!(p.x, 5.0, EPS);
    assert_close!(p.y, 7.0, EPS);
}

#[test]
fn test_transform_point_translation() {
    let m = DMat3::from_array([1.0, 0.0, 10.0, 0.0, 1.0, -5.0, 0.0, 0.0, 1.0]);
    let p = m.transform_point(DVec2::new(3.0, 4.0));
    assert_close!(p.x, 13.0, EPS);
    assert_close!(p.y, -1.0, EPS);
}

#[test]
fn test_transform_point_perspective() {
    let m = DMat3::from_array([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.001, 0.0, 1.0]);
    let p = m.transform_point(DVec2::new(100.0, 0.0));
    // w = 0.001 * 100 + 1 = 1.1
    assert!((p.x - 90.909).abs() < 0.01);
    assert_close!(p.y, 0.0, EPS);
}

#[test]
fn test_transform_point_at_infinity_returns_infinity() {
    // Bottom row [1, 0, -5] gives w = x - 5; at x = 5, w = 0 (point at infinity).
    // Maps to INFINITY (not NaN) so a warp's bounds check rejects it → border.
    let m = DMat3::from_array([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, -5.0]);
    let p = m.transform_point(DVec2::new(5.0, 0.0));
    assert!(p.x.is_infinite() && p.y.is_infinite());
    // inf saturates to i32::MAX (out of bounds), unlike NaN which casts to 0.
    assert_eq!(p.x as i32, i32::MAX);
}

#[test]
fn test_transform_point_roundtrip() {
    let m = DMat3::from_rows([1.1, 0.2, 5.0], [-0.1, 0.9, -3.0], [0.0, 0.0, 1.0]);
    let inv = m.inverse().unwrap();
    let p = DVec2::new(10.0, -5.0);
    let p2 = inv.transform_point(m.transform_point(p));
    assert_close!(p2.x, p.x, EPS);
    assert_close!(p2.y, p.y, EPS);
}

#[test]
fn test_deviation_from_identity_zero() {
    assert_close!(DMat3::identity().deviation_from_identity(), 0.0, EPS);
}

#[test]
fn test_deviation_from_identity_nonzero() {
    let m = DMat3::from_array([1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);
    // Only m[2] differs by 1.0
    assert_close!(m.deviation_from_identity(), 1.0, EPS);
}

#[test]
fn test_deviation_from_identity_multiple_elements() {
    // Diagonal elements differ by 1.0 each: (2-1)^2 + (2-1)^2 + (2-1)^2 = 3
    let m = DMat3::from_rows([2.0, 0.0, 0.0], [0.0, 2.0, 0.0], [0.0, 0.0, 2.0]);
    assert_close!(m.deviation_from_identity(), 3.0_f64.sqrt(), EPS);
}

#[test]
#[should_panic]
fn test_index_out_of_bounds() {
    let m = DMat3::identity();
    let _ = m[9];
}

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
