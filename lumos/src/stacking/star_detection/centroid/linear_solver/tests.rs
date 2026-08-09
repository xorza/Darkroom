use crate::stacking::star_detection::centroid::linear_solver::*;

#[test]
fn solve_5x5_identity() {
    let a = [
        [1.0, 0.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 0.0, 1.0],
    ];
    let b = [1.0, 2.0, 3.0, 4.0, 5.0];

    let x = solve(&a, &b).unwrap();
    for i in 0..5 {
        assert!((x[i] - b[i]).abs() < 1e-12);
    }
}

#[test]
fn solve_5x5_diagonal() {
    let a = [
        [2.0, 0.0, 0.0, 0.0, 0.0],
        [0.0, 3.0, 0.0, 0.0, 0.0],
        [0.0, 0.0, 4.0, 0.0, 0.0],
        [0.0, 0.0, 0.0, 5.0, 0.0],
        [0.0, 0.0, 0.0, 0.0, 6.0],
    ];
    let b = [2.0, 6.0, 12.0, 20.0, 30.0];

    let x = solve(&a, &b).unwrap();
    assert!((x[0] - 1.0).abs() < 1e-12);
    assert!((x[1] - 2.0).abs() < 1e-12);
    assert!((x[2] - 3.0).abs() < 1e-12);
    assert!((x[3] - 4.0).abs() < 1e-12);
    assert!((x[4] - 5.0).abs() < 1e-12);
}

#[test]
fn solve_5x5_singular_returns_none() {
    let a = [[0.0; 5]; 5];
    let b = [1.0, 2.0, 3.0, 4.0, 5.0];
    assert!(solve(&a, &b).is_none());
}

#[test]
fn solve_6x6_identity() {
    let a = [
        [1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0, 0.0, 0.0],
        [0.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 0.0, 0.0, 1.0],
    ];
    let b = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];

    let x = solve(&a, &b).unwrap();
    for i in 0..6 {
        assert!((x[i] - b[i]).abs() < 1e-12);
    }
}

#[test]
fn solve_6x6_diagonal() {
    let a = [
        [2.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        [0.0, 3.0, 0.0, 0.0, 0.0, 0.0],
        [0.0, 0.0, 4.0, 0.0, 0.0, 0.0],
        [0.0, 0.0, 0.0, 5.0, 0.0, 0.0],
        [0.0, 0.0, 0.0, 0.0, 6.0, 0.0],
        [0.0, 0.0, 0.0, 0.0, 0.0, 7.0],
    ];
    let b = [2.0, 6.0, 12.0, 20.0, 30.0, 42.0];

    let x = solve(&a, &b).unwrap();
    assert!((x[0] - 1.0).abs() < 1e-12);
    assert!((x[1] - 2.0).abs() < 1e-12);
    assert!((x[2] - 3.0).abs() < 1e-12);
    assert!((x[3] - 4.0).abs() < 1e-12);
    assert!((x[4] - 5.0).abs() < 1e-12);
    assert!((x[5] - 6.0).abs() < 1e-12);
}

#[test]
fn solve_6x6_singular_returns_none() {
    let a = [[0.0; 6]; 6];
    let b = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
    assert!(solve(&a, &b).is_none());
}

#[test]
fn solve_nan_pivot_returns_none() {
    // A NaN diagonal entry must not slip past the singularity check: every
    // ordered comparison against NaN is false, so a naive `max_val < eps`
    // check would leave `max_row` unmoved and later divide by NaN, returning
    // `Some([NaN, ...])` instead of the documented `None`.
    let mut a = [[0.0; 5]; 5];
    for (i, row) in a.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    a[2][2] = f64::NAN;
    let b = [1.0, 2.0, 3.0, 4.0, 5.0];
    assert!(solve(&a, &b).is_none());
}

#[test]
fn solve_6x6_needs_pivoting() {
    let a = [
        [0.0, 1.0, 0.0, 0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0, 0.0, 0.0],
        [0.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 0.0, 0.0, 1.0],
    ];
    let b = [2.0, 1.0, 3.0, 4.0, 5.0, 6.0];

    let x = solve(&a, &b).unwrap();
    assert!((x[0] - 1.0).abs() < 1e-12);
    assert!((x[1] - 2.0).abs() < 1e-12);
}

#[test]
fn solve_6x6_dense_matrix() {
    // Dense symmetric positive definite matrix (typical for L-M Hessian)
    let a = [
        [10.0, 2.0, 1.0, 0.5, 0.3, 0.1],
        [2.0, 8.0, 1.5, 0.8, 0.4, 0.2],
        [1.0, 1.5, 6.0, 1.0, 0.6, 0.3],
        [0.5, 0.8, 1.0, 5.0, 0.7, 0.4],
        [0.3, 0.4, 0.6, 0.7, 4.0, 0.5],
        [0.1, 0.2, 0.3, 0.4, 0.5, 3.0],
    ];
    let expected_x = [1.0f64, 2.0, 3.0, 4.0, 5.0, 6.0];
    let mut b = [0.0f64; 6];
    for i in 0..6 {
        for j in 0..6 {
            b[i] += a[i][j] * expected_x[j];
        }
    }

    let x = solve(&a, &b).unwrap();
    for i in 0..6 {
        assert!(
            (x[i] - expected_x[i]).abs() < 1e-10,
            "x[{}] = {}, expected {}",
            i,
            x[i],
            expected_x[i]
        );
    }
}

#[test]
fn solve_6x6_verify_solution() {
    // Verify Ax = b holds for the solution
    let a = [
        [4.0, 1.0, 0.0, 0.0, 0.0, 0.0],
        [1.0, 4.0, 1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 4.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 4.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0, 4.0, 1.0],
        [0.0, 0.0, 0.0, 0.0, 1.0, 4.0],
    ];
    let b = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];

    let x = solve(&a, &b).unwrap();

    // Verify: compute A*x and compare to b
    for i in 0..6 {
        let mut ax_i = 0.0f64;
        for j in 0..6 {
            ax_i += a[i][j] * x[j];
        }
        assert!(
            (ax_i - b[i]).abs() < 1e-10,
            "Ax[{}] = {}, expected b[{}] = {}",
            i,
            ax_i,
            i,
            b[i]
        );
    }
}

#[test]
fn solve_5x5_dense_matrix() {
    // Dense matrix for Moffat fitting
    let a = [
        [8.0, 1.5, 1.0, 0.5, 0.2],
        [1.5, 6.0, 1.2, 0.8, 0.3],
        [1.0, 1.2, 5.0, 0.9, 0.4],
        [0.5, 0.8, 0.9, 4.0, 0.5],
        [0.2, 0.3, 0.4, 0.5, 3.0],
    ];
    let expected_x = [1.0f64, 2.0, 3.0, 4.0, 5.0];
    let mut b = [0.0f64; 5];
    for i in 0..5 {
        for j in 0..5 {
            b[i] += a[i][j] * expected_x[j];
        }
    }

    let x = solve(&a, &b).unwrap();
    for i in 0..5 {
        assert!(
            (x[i] - expected_x[i]).abs() < 1e-10,
            "x[{}] = {}, expected {}",
            i,
            x[i],
            expected_x[i]
        );
    }
}
