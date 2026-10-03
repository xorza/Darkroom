use super::*;

#[test]
fn term_exponents_order_2() {
    // Order 2: terms where p+q = 2 (linear terms excluded).
    // p+q=2: (2,0), (1,1), (0,2) = 3 terms.
    let terms = term_exponents(2);
    assert_eq!(terms.len(), 3);
    assert_eq!(terms[0], (2, 0)); // u^2
    assert_eq!(terms[1], (1, 1)); // u*v
    assert_eq!(terms[2], (0, 2)); // v^2
}

#[test]
fn term_exponents_order_3() {
    // Order 3: terms with 2 <= p+q <= 3.
    // p+q=2: (2,0), (1,1), (0,2) = 3 terms
    // p+q=3: (3,0), (2,1), (1,2), (0,3) = 4 terms
    // Total = 7 terms.
    let terms = term_exponents(3);
    assert_eq!(terms.len(), 7);
    // p+q=2 block
    assert_eq!(terms[0], (2, 0));
    assert_eq!(terms[1], (1, 1));
    assert_eq!(terms[2], (0, 2));
    // p+q=3 block
    assert_eq!(terms[3], (3, 0));
    assert_eq!(terms[4], (2, 1));
    assert_eq!(terms[5], (1, 2));
    assert_eq!(terms[6], (0, 3));
}

#[test]
fn term_exponents_order_4() {
    // Order 4: 2 <= p+q <= 4.
    // p+q=2: 3, p+q=3: 4, p+q=4: 5. Total = 12.
    let terms = term_exponents(4);
    assert_eq!(terms.len(), 12);
    // Spot-check the p+q=4 block starts at index 7
    assert_eq!(terms[7], (4, 0));
    assert_eq!(terms[11], (0, 4));
}

#[test]
fn term_exponents_order_5() {
    // Order 5: (5+1)(5+2)/2 - 3 = 21 - 3 = 18 terms.
    // p+q=2: 3, p+q=3: 4, p+q=4: 5, p+q=5: 6. Total = 18.
    let terms = term_exponents(5);
    assert_eq!(terms.len(), 18);
    // Last term should be (0, 5)
    assert_eq!(terms[17], (0, 5));
    // First term of p+q=5 block is at index 12
    assert_eq!(terms[12], (5, 0));
}

#[test]
fn term_exponents_all_satisfy_constraints() {
    for order in 2..=5 {
        let terms = term_exponents(order);
        for &(p, q) in &terms {
            let total = p + q;
            assert!(
                total >= 2 && total <= order,
                "Order {order}: term ({p},{q}) has p+q={total} outside [2,{order}]"
            );
        }
    }
}

/// Every monomial of the order-5 table at `(u, v) = (−2, 3)` against `u^p·v^q` written out:
/// `(−2)³·3 = −24`, `(−2)²·3² = 36`, and so on. Integers this small multiply exactly, so the powers
/// table and the direct products agree bit for bit — and at the origin only `u⁰v⁰` would be 1,
/// which no SIP term is.
#[test]
fn basis_is_every_term_at_the_point() {
    let terms = term_exponents(5);
    let mut basis = [0.0; MAX_TERMS];
    evaluate_basis(DVec2::new(-2.0, 3.0), &terms, &mut basis[..terms.len()]);
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

    evaluate_basis(DVec2::ZERO, &terms, &mut basis[..terms.len()]);
    assert!(basis[..terms.len()].iter().all(|&value| value == 0.0));
}
