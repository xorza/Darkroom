use crate::stacking::registration::distortion::sip::tests::*;

#[test]
fn test_solve_cholesky_2x2_hand_computed() {
    // Solve [4 2; 2 3] * x = [8; 7]
    // By hand: det = 12-4 = 8
    // x = [4 2; 2 3]^-1 * [8; 7] = (1/8)*[3 -2; -2 4]*[8; 7] = (1/8)*[10; 12] = [1.25, 1.5]
    let n = 2;
    let mut a = [0.0; MAX_ATA];
    a[0] = 4.0;
    a[1] = 2.0;
    a[2] = 2.0; // a[1*n+0]
    a[3] = 3.0; // a[1*n+1]

    let mut b = [0.0; MAX_TERMS];
    b[0] = 8.0;
    b[1] = 7.0;

    let x = solve_cholesky(&a, &b, n).unwrap();
    assert_eq!(x.len(), 2);
    assert!(
        (x[0] - 1.25).abs() < 1e-12,
        "x[0] = {}, expected 1.25",
        x[0]
    );
    assert!((x[1] - 1.5).abs() < 1e-12, "x[1] = {}, expected 1.5", x[1]);
}

#[test]
fn test_solve_cholesky_3x3_identity() {
    // Solve I * x = [3, 5, 7] => x = [3, 5, 7]
    let n = 3;
    let mut a = [0.0; MAX_ATA];
    a[0] = 1.0; // (0,0)
    a[n + 1] = 1.0; // (1,1)
    a[2 * n + 2] = 1.0; // (2,2)

    let mut b = [0.0; MAX_TERMS];
    b[0] = 3.0;
    b[1] = 5.0;
    b[2] = 7.0;

    let x = solve_cholesky(&a, &b, n).unwrap();
    assert_eq!(x.len(), 3);
    assert!((x[0] - 3.0).abs() < 1e-12);
    assert!((x[1] - 5.0).abs() < 1e-12);
    assert!((x[2] - 7.0).abs() < 1e-12);
}

#[test]
fn test_solve_lu_2x2_hand_computed() {
    // Same problem as Cholesky test but using LU directly.
    // [4 2; 2 3] * x = [8; 7] => x = [1.25, 1.5]
    let n = 2;
    let mut a = [0.0; MAX_ATA];
    a[0] = 4.0;
    a[1] = 2.0;
    a[2] = 2.0;
    a[3] = 3.0;

    let mut b = [0.0; MAX_TERMS];
    b[0] = 8.0;
    b[1] = 7.0;

    let x = solve_lu(&a, &b, n).unwrap();
    assert_eq!(x.len(), 2);
    assert!(
        (x[0] - 1.25).abs() < 1e-12,
        "x[0] = {}, expected 1.25",
        x[0]
    );
    assert!((x[1] - 1.5).abs() < 1e-12, "x[1] = {}, expected 1.5", x[1]);
}

#[test]
fn test_solve_lu_singular_returns_none() {
    // Singular matrix: [1 2; 2 4] (row 2 = 2 * row 1)
    let n = 2;
    let mut a = [0.0; MAX_ATA];
    a[0] = 1.0;
    a[1] = 2.0;
    a[2] = 2.0;
    a[3] = 4.0;

    let mut b = [0.0; MAX_TERMS];
    b[0] = 1.0;
    b[1] = 2.0;

    assert!(solve_lu(&a, &b, n).is_none());
}

#[test]
fn test_solve_lu_needs_pivoting() {
    // Matrix where first diagonal is zero, requiring pivoting:
    // [0 1; 1 0] * x = [3; 5] => x = [5, 3]
    let n = 2;
    let mut a = [0.0; MAX_ATA];
    a[0] = 0.0;
    a[1] = 1.0;
    a[2] = 1.0;
    a[3] = 0.0;

    let mut b = [0.0; MAX_TERMS];
    b[0] = 3.0;
    b[1] = 5.0;

    let x = solve_lu(&a, &b, n).unwrap();
    assert_eq!(x.len(), 2);
    assert!((x[0] - 5.0).abs() < 1e-12, "x[0] = {}, expected 5.0", x[0]);
    assert!((x[1] - 3.0).abs() < 1e-12, "x[1] = {}, expected 3.0", x[1]);
}

#[test]
fn test_cholesky_falls_back_to_lu_for_non_positive_definite() {
    // Not positive definite: [1 0; 0 -1]. Cholesky should fail on diag,
    // then fall back to LU.
    // [1 0; 0 -1] * x = [2; -3] => x = [2, 3]
    let n = 2;
    let mut a = [0.0; MAX_ATA];
    a[0] = 1.0;
    a[1] = 0.0;
    a[2] = 0.0;
    a[3] = -1.0;

    let mut b = [0.0; MAX_TERMS];
    b[0] = 2.0;
    b[1] = -3.0;

    let x = solve_cholesky(&a, &b, n).unwrap();
    assert_eq!(x.len(), 2);
    assert!((x[0] - 2.0).abs() < 1e-12, "x[0] = {}, expected 2.0", x[0]);
    assert!((x[1] - 3.0).abs() < 1e-12, "x[1] = {}, expected 3.0", x[1]);
}
