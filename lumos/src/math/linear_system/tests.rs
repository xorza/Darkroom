//! Tests for the dense solver, gathered from the three implementations it replaced.

use crate::math::linear_system::solve_in_place;

/// The distortion solvers' threshold.
const COARSE: f64 = 1e-12;
/// The Levenberg-Marquardt step's threshold.
const FINE: f64 = 1e-15;

/// Solve a copy, so a case can be written as rows and read as a solution.
fn solved<const N: usize>(a: [[f64; N]; N], b: [f64; N], singular_below: f64) -> Option<[f64; N]> {
    let mut matrix = a;
    let mut rhs = b;
    solve_in_place(matrix.as_flattened_mut(), &mut rhs, singular_below)?;
    Some(rhs)
}

/// `A · x` for checking a recovered solution against the right-hand side it came from.
fn multiply<const N: usize>(a: &[[f64; N]; N], x: &[f64; N]) -> [f64; N] {
    let mut out = [0.0; N];
    for (row, value) in a.iter().zip(out.iter_mut()) {
        *value = row.iter().zip(x).map(|(a, x)| a * x).sum();
    }
    out
}

/// Systems whose solution is known by construction, at both sizes the LM step uses and with the
/// two structures elimination has to handle: a zero leading pivot, and a dense row.
#[test]
fn known_systems_are_solved_exactly() {
    let identity5 = [
        [1.0, 0.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 0.0, 1.0],
    ];
    let b = [1.0, 2.0, 3.0, 4.0, 5.0];
    assert_eq!(solved(identity5, b, FINE).unwrap(), b, "identity returns b");

    // Diagonal: xᵢ = bᵢ / aᵢᵢ, each chosen to divide exactly.
    let mut diagonal6 = [[0.0f64; 6]; 6];
    for (i, row) in diagonal6.iter_mut().enumerate() {
        row[i] = i as f64 + 2.0;
    }
    let scaled = [2.0, 6.0, 12.0, 20.0, 30.0, 42.0];
    assert_eq!(
        solved(diagonal6, scaled, FINE).unwrap(),
        [1.0, 2.0, 3.0, 4.0, 5.0, 6.0],
        "diagonal divides through"
    );

    // A zero at [0][0] forces a row swap before elimination can start; the swap has to carry the
    // right-hand side with it, so a solver that swapped only `a` would return [1, 2] here.
    let swapped = [[0.0, 1.0], [1.0, 0.0]];
    assert_eq!(solved(swapped, [2.0, 1.0], COARSE).unwrap(), [1.0, 2.0]);

    // 2×2 with a hand-derived answer: 2x + y = 5, x + 3y = 10 → x = 1, y = 3.
    let dense2 = [[2.0, 1.0], [1.0, 3.0]];
    let x = solved(dense2, [5.0, 10.0], COARSE).unwrap();
    assert!(
        (x[0] - 1.0).abs() < 1e-12 && (x[1] - 3.0).abs() < 1e-12,
        "{x:?}"
    );
}

/// A dense symmetric system at each size, checked by recovering the `x` its `b` was built from —
/// the shape the LM step actually feeds in (a damped Hessian).
#[test]
fn a_dense_symmetric_system_recovers_the_x_its_b_was_built_from() {
    let hessian6 = [
        [10.0, 2.0, 1.0, 0.5, 0.3, 0.1],
        [2.0, 8.0, 1.5, 0.8, 0.4, 0.2],
        [1.0, 1.5, 6.0, 1.0, 0.6, 0.3],
        [0.5, 0.8, 1.0, 5.0, 0.7, 0.4],
        [0.3, 0.4, 0.6, 0.7, 4.0, 0.5],
        [0.1, 0.2, 0.3, 0.4, 0.5, 3.0],
    ];
    let expected = [1.0f64, 2.0, 3.0, 4.0, 5.0, 6.0];
    let solution = solved(hessian6, multiply(&hessian6, &expected), FINE).unwrap();
    for (got, want) in solution.iter().zip(&expected) {
        assert!((got - want).abs() < 1e-10, "{solution:?} vs {expected:?}");
    }

    let hessian5 = [
        [8.0, 1.5, 1.0, 0.5, 0.2],
        [1.5, 6.0, 1.2, 0.8, 0.3],
        [1.0, 1.2, 5.0, 0.9, 0.4],
        [0.5, 0.8, 0.9, 4.0, 0.5],
        [0.2, 0.3, 0.4, 0.5, 3.0],
    ];
    let expected = [1.0f64, 2.0, 3.0, 4.0, 5.0];
    let solution = solved(hessian5, multiply(&hessian5, &expected), FINE).unwrap();
    for (got, want) in solution.iter().zip(&expected) {
        assert!((got - want).abs() < 1e-10, "{solution:?} vs {expected:?}");
    }

    // Tridiagonal, checked the other way round: substitute the solution back and compare to `b`.
    let mut tridiagonal = [[0.0f64; 6]; 6];
    for i in 0..6 {
        tridiagonal[i][i] = 4.0;
        if i > 0 {
            tridiagonal[i][i - 1] = 1.0;
            tridiagonal[i - 1][i] = 1.0;
        }
    }
    let b = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
    let residual = multiply(&tridiagonal, &solved(tridiagonal, b, COARSE).unwrap());
    for (got, want) in residual.iter().zip(&b) {
        assert!((got - want).abs() < 1e-10, "A·x = {residual:?}, b = {b:?}");
    }
}

/// Every way the matrix can fail to have a unique solution, at both thresholds.
#[test]
fn singular_systems_return_none() {
    let b5 = [1.0, 2.0, 3.0, 4.0, 5.0];
    assert!(solved([[0.0; 5]; 5], b5, FINE).is_none(), "all zeros");
    assert!(
        solved([[0.0; 6]; 6], [1.0; 6], COARSE).is_none(),
        "all zeros, 6×6"
    );

    // Rank deficiency that only surfaces mid-elimination: rows 1 and 2 are identical, so the
    // second pivot column is all zeros only after the first column has been eliminated.
    let duplicate = [[1.0, 2.0, 3.0], [2.0, 4.0, 6.0], [0.0, 0.0, 1.0]];
    assert!(solved(duplicate, [1.0, 2.0, 3.0], COARSE).is_none());

    // A NaN pivot fails every ordered comparison, so `pivot < singular_below` alone would pass it
    // through and divide by it.
    let mut nan_diagonal = [[0.0; 5]; 5];
    for (i, row) in nan_diagonal.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    nan_diagonal[2][2] = f64::NAN;
    assert!(solved(nan_diagonal, b5, FINE).is_none(), "NaN pivot");
}

/// The threshold is the caller's to set, and it decides: a pivot of 1e-13 is a solvable system to
/// the LM step and a singular one to the distortion fits.
#[test]
fn the_pivot_threshold_decides_which_systems_are_singular() {
    let nearly = [[1e-13, 0.0], [0.0, 1.0]];
    let b = [1e-13, 2.0];

    assert_eq!(solved(nearly, b, FINE).unwrap(), [1.0, 2.0]);
    assert!(solved(nearly, b, COARSE).is_none());
}

/// The solution lands in `b` and the matrix is spent — the contract that lets the caller own the
/// storage.
#[test]
fn both_operands_are_consumed() {
    let mut a = [2.0, 0.0, 0.0, 4.0];
    let mut b = [6.0, 8.0];
    solve_in_place(&mut a, &mut b, FINE).unwrap();
    assert_eq!(b, [3.0, 2.0], "x = [6/2, 8/4]");

    // 1×1 is the degenerate case the loops have to survive: no elimination, one division.
    let mut a = [4.0];
    let mut b = [10.0];
    solve_in_place(&mut a, &mut b, FINE).unwrap();
    assert_eq!(b, [2.5]);
}
