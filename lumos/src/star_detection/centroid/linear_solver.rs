//! Linear system solvers for profile fitting.
//!
//! Provides Gaussian elimination with partial pivoting for small dense
//! linear systems used in Levenberg-Marquardt optimization.

/// Solve NxN linear system using Gaussian elimination with partial pivoting.
///
/// Solves the system Ax = b for x.
/// Returns None if the matrix is singular (pivot too small).
///
/// Works for small fixed-size systems (N <= 6).
#[inline]
#[allow(clippy::needless_range_loop)]
fn solve_generic<const N: usize>(a: &[[f32; N]; N], b: &[f32; N]) -> Option<[f32; N]> {
    let mut matrix = *a;
    let mut rhs = *b;

    // Forward elimination with partial pivoting
    for col in 0..N {
        // Find pivot
        let mut max_row = col;
        let mut max_val = matrix[col][col].abs();
        for row in (col + 1)..N {
            let val = matrix[row][col].abs();
            if val > max_val {
                max_val = val;
                max_row = row;
            }
        }

        if max_val < 1e-10 {
            return None; // Singular matrix
        }

        // Swap rows
        if max_row != col {
            matrix.swap(col, max_row);
            rhs.swap(col, max_row);
        }

        // Precompute pivot reciprocal to avoid repeated divisions
        let inv_pivot = 1.0 / matrix[col][col];

        // Eliminate column
        for row in (col + 1)..N {
            let factor = matrix[row][col] * inv_pivot;
            // Skip col position (becomes zero) - start from col+1
            for j in (col + 1)..N {
                matrix[row][j] -= factor * matrix[col][j];
            }
            rhs[row] -= factor * rhs[col];
        }
    }

    // Back substitution
    let mut x = [0.0f32; N];
    for i in (0..N).rev() {
        let mut sum = rhs[i];
        for j in (i + 1)..N {
            sum -= matrix[i][j] * x[j];
        }
        x[i] = sum / matrix[i][i];
    }

    Some(x)
}

/// Solve 5x5 linear system.
#[inline]
pub fn solve_5x5(a: &[[f32; 5]; 5], b: &[f32; 5]) -> Option<[f32; 5]> {
    solve_generic(a, b)
}

/// Solve 6x6 linear system.
#[inline]
pub fn solve_6x6(a: &[[f32; 6]; 6], b: &[f32; 6]) -> Option<[f32; 6]> {
    solve_generic(a, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_solve_5x5_identity() {
        let a = [
            [1.0, 0.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 0.0, 1.0],
        ];
        let b = [1.0, 2.0, 3.0, 4.0, 5.0];

        let x = solve_5x5(&a, &b).unwrap();
        for i in 0..5 {
            assert!((x[i] - b[i]).abs() < 1e-6);
        }
    }

    #[test]
    fn test_solve_5x5_diagonal() {
        let a = [
            [2.0, 0.0, 0.0, 0.0, 0.0],
            [0.0, 3.0, 0.0, 0.0, 0.0],
            [0.0, 0.0, 4.0, 0.0, 0.0],
            [0.0, 0.0, 0.0, 5.0, 0.0],
            [0.0, 0.0, 0.0, 0.0, 6.0],
        ];
        let b = [2.0, 6.0, 12.0, 20.0, 30.0];

        let x = solve_5x5(&a, &b).unwrap();
        assert!((x[0] - 1.0).abs() < 1e-6);
        assert!((x[1] - 2.0).abs() < 1e-6);
        assert!((x[2] - 3.0).abs() < 1e-6);
        assert!((x[3] - 4.0).abs() < 1e-6);
        assert!((x[4] - 5.0).abs() < 1e-6);
    }

    #[test]
    fn test_solve_5x5_singular_returns_none() {
        let a = [[0.0; 5]; 5];
        let b = [1.0, 2.0, 3.0, 4.0, 5.0];
        assert!(solve_5x5(&a, &b).is_none());
    }

    #[test]
    fn test_solve_6x6_identity() {
        let a = [
            [1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0, 0.0, 0.0],
            [0.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 0.0, 0.0, 1.0],
        ];
        let b = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];

        let x = solve_6x6(&a, &b).unwrap();
        for i in 0..6 {
            assert!((x[i] - b[i]).abs() < 1e-6);
        }
    }

    #[test]
    fn test_solve_6x6_diagonal() {
        let a = [
            [2.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            [0.0, 3.0, 0.0, 0.0, 0.0, 0.0],
            [0.0, 0.0, 4.0, 0.0, 0.0, 0.0],
            [0.0, 0.0, 0.0, 5.0, 0.0, 0.0],
            [0.0, 0.0, 0.0, 0.0, 6.0, 0.0],
            [0.0, 0.0, 0.0, 0.0, 0.0, 7.0],
        ];
        let b = [2.0, 6.0, 12.0, 20.0, 30.0, 42.0];

        let x = solve_6x6(&a, &b).unwrap();
        assert!((x[0] - 1.0).abs() < 1e-6);
        assert!((x[1] - 2.0).abs() < 1e-6);
        assert!((x[2] - 3.0).abs() < 1e-6);
        assert!((x[3] - 4.0).abs() < 1e-6);
        assert!((x[4] - 5.0).abs() < 1e-6);
        assert!((x[5] - 6.0).abs() < 1e-6);
    }

    #[test]
    fn test_solve_6x6_singular_returns_none() {
        let a = [[0.0; 6]; 6];
        let b = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        assert!(solve_6x6(&a, &b).is_none());
    }

    #[test]
    fn test_solve_6x6_needs_pivoting() {
        let a = [
            [0.0, 1.0, 0.0, 0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0, 0.0, 0.0],
            [0.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 0.0, 0.0, 1.0],
        ];
        let b = [2.0, 1.0, 3.0, 4.0, 5.0, 6.0];

        let x = solve_6x6(&a, &b).unwrap();
        assert!((x[0] - 1.0).abs() < 1e-6);
        assert!((x[1] - 2.0).abs() < 1e-6);
    }

    #[test]
    fn test_solve_6x6_dense_matrix() {
        // Dense symmetric positive definite matrix (typical for L-M Hessian)
        let a = [
            [10.0, 2.0, 1.0, 0.5, 0.3, 0.1],
            [2.0, 8.0, 1.5, 0.8, 0.4, 0.2],
            [1.0, 1.5, 6.0, 1.0, 0.6, 0.3],
            [0.5, 0.8, 1.0, 5.0, 0.7, 0.4],
            [0.3, 0.4, 0.6, 0.7, 4.0, 0.5],
            [0.1, 0.2, 0.3, 0.4, 0.5, 3.0],
        ];
        // Expected solution x = [1, 2, 3, 4, 5, 6]
        // Compute b = A * x
        let expected_x = [1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0];
        let mut b = [0.0f32; 6];
        for i in 0..6 {
            for j in 0..6 {
                b[i] += a[i][j] * expected_x[j];
            }
        }

        let x = solve_6x6(&a, &b).unwrap();
        for i in 0..6 {
            assert!(
                (x[i] - expected_x[i]).abs() < 1e-4,
                "x[{}] = {}, expected {}",
                i,
                x[i],
                expected_x[i]
            );
        }
    }

    #[test]
    fn test_solve_6x6_verify_solution() {
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

        let x = solve_6x6(&a, &b).unwrap();

        // Verify: compute A*x and compare to b
        for i in 0..6 {
            let mut ax_i = 0.0f32;
            for j in 0..6 {
                ax_i += a[i][j] * x[j];
            }
            assert!(
                (ax_i - b[i]).abs() < 1e-5,
                "Ax[{}] = {}, expected b[{}] = {}",
                i,
                ax_i,
                i,
                b[i]
            );
        }
    }

    #[test]
    fn test_solve_5x5_dense_matrix() {
        // Dense matrix for Moffat fitting
        let a = [
            [8.0, 1.5, 1.0, 0.5, 0.2],
            [1.5, 6.0, 1.2, 0.8, 0.3],
            [1.0, 1.2, 5.0, 0.9, 0.4],
            [0.5, 0.8, 0.9, 4.0, 0.5],
            [0.2, 0.3, 0.4, 0.5, 3.0],
        ];
        let expected_x = [1.0f32, 2.0, 3.0, 4.0, 5.0];
        let mut b = [0.0f32; 5];
        for i in 0..5 {
            for j in 0..5 {
                b[i] += a[i][j] * expected_x[j];
            }
        }

        let x = solve_5x5(&a, &b).unwrap();
        for i in 0..5 {
            assert!(
                (x[i] - expected_x[i]).abs() < 1e-4,
                "x[{}] = {}, expected {}",
                i,
                x[i],
                expected_x[i]
            );
        }
    }
}
