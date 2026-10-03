use crate::math::dmat3::*;

fn from_rows(row0: [f64; 3], row1: [f64; 3], row2: [f64; 3]) -> DMat3 {
    DMat3::from_array([
        row0[0], row0[1], row0[2], row1[0], row1[1], row1[2], row2[0], row2[1], row2[2],
    ])
}

/// Every way in and every way out of a `DMat3`, against one asymmetric array — asymmetric so a
/// transposed or mis-strided accessor cannot round-trip by accident.
#[test]
fn every_constructor_and_accessor_round_trips() {
    const DATA: [f64; 9] = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0];

    for m in [
        DMat3::from_array(DATA),
        from_rows([1.0, 2.0, 3.0], [4.0, 5.0, 6.0], [7.0, 8.0, 9.0]),
    ] {
        assert_eq!(*m.as_array(), DATA);
        // Indexing is row-major over the same flat storage.
        for (i, expected) in DATA.iter().enumerate() {
            assert_eq!(m[i], *expected, "index {i}");
        }
    }
}

/// Mutable indexing reaches the same storage the read paths do.
#[test]
fn mutable_accessors_write_through() {
    let mut m = DMat3::identity();
    m[2] = 5.0;
    m[5] = -3.0;
    assert_eq!(
        *m.as_array(),
        [1.0, 0.0, 5.0, 0.0, 1.0, -3.0, 0.0, 0.0, 1.0]
    );
}

/// Every fixture below is small integers, dyadic fractions or a power of two, so each product and
/// sum is exact in f64 and every result compares with `assert_eq!`.
const M: [f64; 9] = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0];

/// `identity` is the multiplicative identity on both sides.
#[test]
fn identity_is_the_multiplicative_identity_and_the_default() {
    let identity = DMat3::identity();
    assert_eq!(
        *identity.as_array(),
        [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]
    );
    let m = DMat3::from_array(M);
    assert_eq!(m.mul_mat(&identity), m);
    assert_eq!(identity.mul_mat(&m), m);
}

/// The cofactor expansion: 1·(2·6 − 3·5) − 2·(1·6 − 3·4) + 3·(1·5 − 2·4) = −3 + 12 − 9 = 0 for two
/// equal rows; the product of a diagonal; and −1 for a row swap.
#[test]
fn determinant_hand_computed() {
    for (m, det) in [
        (DMat3::identity(), 1.0),
        (
            from_rows([1.0, 2.0, 3.0], [1.0, 2.0, 3.0], [4.0, 5.0, 6.0]),
            0.0,
        ),
        (
            from_rows([2.0, 0.0, 0.0], [0.0, 3.0, 0.0], [0.0, 0.0, 4.0]),
            24.0,
        ),
        (
            from_rows([0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
            -1.0,
        ),
    ] {
        assert_eq!(m.determinant(), det, "{m:?}");
    }
}

/// Inverses on both sides of the singularity threshold `1e-12 · min(scale³, 1)`:
/// - `2⁻¹⁷·I` is perfectly conditioned, det 2⁻⁵¹ ≈ 4.4e-16 — under a fixed 1e-12, over the
///   threshold scaled down to 1e-12·2⁻⁵¹ — and inverts to 2¹⁷·I;
/// - a translation of 1e10 has det 1 and scale³ 1e30, which only the cap at 1 keeps invertible;
/// - `[1 2 3; 0 1 4; 5 6 0]` has det 1, so its inverse is integers and `M·M⁻¹ = I` exactly.
#[test]
fn inverse_on_both_sides_of_the_threshold() {
    let tiny = 2.0f64.powi(-17);
    let huge = 2.0f64.powi(17);
    let m = from_rows([1.0, 2.0, 3.0], [0.0, 1.0, 4.0], [5.0, 6.0, 0.0]);
    for (m, inverse) in [
        (DMat3::identity(), DMat3::identity()),
        (
            from_rows([2.0, 0.0, 0.0], [0.0, 4.0, 0.0], [0.0, 0.0, 8.0]),
            from_rows([0.5, 0.0, 0.0], [0.0, 0.25, 0.0], [0.0, 0.0, 0.125]),
        ),
        (
            from_rows([tiny, 0.0, 0.0], [0.0, tiny, 0.0], [0.0, 0.0, tiny]),
            from_rows([huge, 0.0, 0.0], [0.0, huge, 0.0], [0.0, 0.0, huge]),
        ),
        (
            from_rows([1.0, 0.0, 1e10], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
            from_rows([1.0, 0.0, -1e10], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
        ),
        (
            m,
            from_rows([-24.0, 18.0, 5.0], [20.0, -15.0, -4.0], [-5.0, 4.0, 1.0]),
        ),
    ] {
        assert_eq!(m.inverse(), Some(inverse), "{m:?}");
    }
    assert_eq!(m.mul_mat(&m.inverse().unwrap()), DMat3::identity());

    // Rank-deficient, with elements large enough (scale³ = 1e9) that a threshold not capped
    // from below would wave it through.
    for singular in [
        DMat3::from_array([0.0; 9]),
        from_rows([1e3, 0.0, 0.0], [0.0, 1e3, 0.0], [1e3, 0.0, 0.0]),
    ] {
        assert_eq!(singular.inverse(), None, "{singular:?}");
    }
}

/// The product, row by column: row 0 of the first pair is
/// [1·1 + 2·0, 1·0 + 2·1, 1·3 + 2·4 + 0·1] = [1, 2, 11]. A shear and its transpose do not commute.
#[test]
fn product_hand_computed() {
    for (a, b, product) in [
        (
            from_rows([1.0, 2.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
            from_rows([1.0, 0.0, 3.0], [0.0, 1.0, 4.0], [0.0, 0.0, 1.0]),
            from_rows([1.0, 2.0, 11.0], [0.0, 1.0, 4.0], [0.0, 0.0, 1.0]),
        ),
        (
            from_rows([2.0, 0.0, 0.0], [0.0, 3.0, 0.0], [0.0, 0.0, 1.0]),
            from_rows([1.0, 0.0, 5.0], [0.0, 1.0, 7.0], [0.0, 0.0, 1.0]),
            from_rows([2.0, 0.0, 10.0], [0.0, 3.0, 21.0], [0.0, 0.0, 1.0]),
        ),
    ] {
        assert_eq!(a.mul_mat(&b), product);
    }
    let a = from_rows([1.0, 2.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]);
    let b = from_rows([1.0, 0.0, 0.0], [2.0, 1.0, 0.0], [0.0, 0.0, 1.0]);
    assert_ne!(a.mul_mat(&b), b.mul_mat(&a));
}

/// Points through the identity, a translation, and a perspective row: `w = 0.25·4 + 1 = 2` halves
/// (4, 6) to (2, 3). An affine map and its inverse — det 8, so dyadic — bring a point back exactly.
#[test]
fn transform_point_hand_computed() {
    for (m, point, image) in [
        (
            DMat3::identity(),
            DVec2::new(5.0, 7.0),
            DVec2::new(5.0, 7.0),
        ),
        (
            from_rows([1.0, 0.0, 10.0], [0.0, 1.0, -5.0], [0.0, 0.0, 1.0]),
            DVec2::new(3.0, 4.0),
            DVec2::new(13.0, -1.0),
        ),
        (
            from_rows([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.25, 0.0, 1.0]),
            DVec2::new(4.0, 6.0),
            DVec2::new(2.0, 3.0),
        ),
    ] {
        assert_eq!(m.transform_point(point), image, "{m:?}");
    }

    let m = from_rows([2.0, 0.0, 5.0], [0.0, 4.0, -3.0], [0.0, 0.0, 1.0]);
    let p = DVec2::new(10.0, -5.0);
    assert_eq!(
        m.inverse().unwrap().transform_point(m.transform_point(p)),
        p
    );
}

#[test]
fn transform_point_at_infinity_returns_infinity() {
    // Bottom row [1, 0, -5] gives w = x - 5; at x = 5, w = 0 (point at infinity).
    // Maps to INFINITY (not NaN) so a warp's bounds check rejects it → border.
    let m = DMat3::from_array([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, -5.0]);
    let p = m.transform_point(DVec2::new(5.0, 0.0));
    assert!(p.x.is_infinite() && p.y.is_infinite());
    // inf saturates to i32::MAX (out of bounds), unlike NaN which casts to 0.
    assert_eq!(p.x as i32, i32::MAX);
}
