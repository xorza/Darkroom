use crate::stacking::registration::distortion::sip::tests::*;

#[test]
fn test_term_exponents_order_2() {
    // Order 2: terms where p+q = 2 (linear terms excluded).
    // p+q=2: (2,0), (1,1), (0,2) = 3 terms.
    let terms = term_exponents(2);
    assert_eq!(terms.len(), 3);
    assert_eq!(terms[0], (2, 0)); // u^2
    assert_eq!(terms[1], (1, 1)); // u*v
    assert_eq!(terms[2], (0, 2)); // v^2
}

#[test]
fn test_term_exponents_order_3() {
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
fn test_term_exponents_order_4() {
    // Order 4: 2 <= p+q <= 4.
    // p+q=2: 3, p+q=3: 4, p+q=4: 5. Total = 12.
    let terms = term_exponents(4);
    assert_eq!(terms.len(), 12);
    // Spot-check the p+q=4 block starts at index 7
    assert_eq!(terms[7], (4, 0));
    assert_eq!(terms[11], (0, 4));
}

#[test]
fn test_term_exponents_order_5() {
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
fn test_term_exponents_all_satisfy_constraints() {
    for order in 2..=5 {
        let terms = term_exponents(order);
        for &(p, q) in terms.iter() {
            let total = p + q;
            assert!(
                total >= 2 && total <= order,
                "Order {}: term ({},{}) has p+q={} outside [2,{}]",
                order,
                p,
                q,
                total,
                order
            );
        }
    }
}

#[test]
fn test_monomial_hand_computed() {
    // u^2 * v^0 = u^2
    // u=3.0, v=5.0: 3^2 * 5^0 = 9.0 * 1.0 = 9.0
    assert_eq!(monomial(3.0, 5.0, 2, 0), 9.0);

    // u^0 * v^3 = v^3
    // u=3.0, v=2.0: 3^0 * 2^3 = 1.0 * 8.0 = 8.0
    assert_eq!(monomial(3.0, 2.0, 0, 3), 8.0);

    // u^1 * v^1 = u*v
    // u=4.0, v=7.0: 4 * 7 = 28.0
    assert_eq!(monomial(4.0, 7.0, 1, 1), 28.0);

    // u^3 * v^2
    // u=2.0, v=3.0: 8.0 * 9.0 = 72.0
    assert_eq!(monomial(2.0, 3.0, 3, 2), 72.0);

    // u^0 * v^0 = 1.0 for any (u, v)
    assert_eq!(monomial(42.0, 99.0, 0, 0), 1.0);
}

#[test]
fn test_monomial_zero_input() {
    // u=0, v=0: u^p * v^q = 0 for any p>0 or q>0
    assert_eq!(monomial(0.0, 0.0, 2, 0), 0.0);
    assert_eq!(monomial(0.0, 0.0, 0, 2), 0.0);
    assert_eq!(monomial(0.0, 0.0, 1, 1), 0.0);
    // u^0 * v^0 = 1.0 even at origin
    assert_eq!(monomial(0.0, 0.0, 0, 0), 1.0);
}

#[test]
fn test_monomial_negative_input() {
    // u=-2.0, v=3.0, p=3, q=1
    // (-2)^3 * 3^1 = -8 * 3 = -24.0
    assert_eq!(monomial(-2.0, 3.0, 3, 1), -24.0);

    // u=-2.0, v=-3.0, p=2, q=2
    // (-2)^2 * (-3)^2 = 4 * 9 = 36.0
    assert_eq!(monomial(-2.0, -3.0, 2, 2), 36.0);
}

#[test]
fn test_avg_distance_hand_computed() {
    let ref_pt = DVec2::new(0.0, 0.0);
    let points = [
        DVec2::new(3.0, 4.0),  // distance = sqrt(9+16) = 5.0
        DVec2::new(0.0, 10.0), // distance = 10.0
        DVec2::new(5.0, 0.0),  // distance = 5.0
    ];
    // avg = (5 + 10 + 5) / 3 = 20/3 = 6.666...
    let avg = avg_distance(&points, ref_pt);
    assert!((avg - 20.0 / 3.0).abs() < 1e-12);
}

#[test]
fn test_avg_distance_all_at_ref_returns_one() {
    // When all points coincide with reference, avg distance = 0 -> clamp to 1.0
    let ref_pt = DVec2::new(5.0, 5.0);
    let points = [ref_pt, ref_pt, ref_pt];
    assert_eq!(avg_distance(&points, ref_pt), 1.0);
}

#[test]
fn test_avg_distance_single_point() {
    // Single point at (6, 8) from origin (0,0): distance = sqrt(36+64) = 10.0
    // avg = 10.0 / 1 = 10.0
    let ref_pt = DVec2::ZERO;
    let points = [DVec2::new(6.0, 8.0)];
    assert!((avg_distance(&points, ref_pt) - 10.0).abs() < 1e-12);
}
