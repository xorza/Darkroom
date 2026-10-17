use super::*;

/// Every monomial of the order-5 table at `(u, v) = (−2, 3)` against `u^p·v^q` written out:
/// `(−2)³·3 = −24`, `(−2)²·3² = 36`, and so on. Integers this small multiply exactly, so the powers
/// table and the direct products agree bit for bit — and at the origin only `u⁰v⁰` would be 1,
/// which no SIP term is.
#[test]
fn basis_is_every_term_at_the_point() {
    let terms = SipOrder::Five.terms();
    let mut basis = [0.0; MAX_TERMS];
    evaluate_basis(DVec2::new(-2.0, 3.0), terms, &mut basis[..terms.len()]);
    for (&(p, q), &value) in terms.iter().zip(&basis) {
        let expected = (-2.0f64).powi(p as i32) * 3.0f64.powi(q as i32);
        assert_eq!(value, expected, "u^{p}·v^{q}");
    }
    assert_eq!(
        basis[terms.iter().position(|&t| t == (3, 1)).unwrap()],
        -24.0
    );
    assert_eq!(
        basis[terms.iter().position(|&t| t == (2, 2)).unwrap()],
        36.0
    );

    evaluate_basis(DVec2::ZERO, terms, &mut basis[..terms.len()]);
    assert!(basis[..terms.len()].iter().all(|&value| value == 0.0));
}
