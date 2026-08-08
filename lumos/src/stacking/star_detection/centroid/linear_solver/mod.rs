//! Linear system solvers for profile fitting.
//!
//! Provides Gaussian elimination with partial pivoting for small dense
//! linear systems used in Levenberg-Marquardt optimization.
//! Uses f64 for numerical stability in the solve path.

/// Solve NxN linear system using Gaussian elimination with partial pivoting.
///
/// Solves the system Ax = b for x.
/// Returns None if the matrix is singular (pivot too small).
///
/// Works for small fixed-size systems (N <= 6).
#[inline]
#[allow(clippy::needless_range_loop)]
pub(super) fn solve<const N: usize>(a: &[[f64; N]; N], b: &[f64; N]) -> Option<[f64; N]> {
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

        // Explicit is_nan() check (rather than plain `max_val < 1e-15`) since a NaN pivot
        // fails every ordered comparison and would otherwise sail through as "not too
        // small", propagating through the solve as `1.0/NaN` instead of returning None.
        if max_val.is_nan() || max_val < 1e-15 {
            return None; // Singular or non-finite matrix
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
    let mut x = [0.0f64; N];
    for i in (0..N).rev() {
        let mut sum = rhs[i];
        for j in (i + 1)..N {
            sum -= matrix[i][j] * x[j];
        }
        x[i] = sum / matrix[i][i];
    }

    Some(x)
}

#[cfg(test)]
mod tests;
