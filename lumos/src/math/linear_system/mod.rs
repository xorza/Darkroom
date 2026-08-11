//! Solving a small dense `A·x = b` by Gaussian elimination with partial pivoting.
//!
//! One routine for every dense solve in the crate: the Levenberg-Marquardt step in star fitting
//! (5×5 and 6×6, per iteration per star), the SIP polynomial fallback when the normal equations
//! aren't positive definite (up to 18×18), and the thin-plate-spline system (control points + 3).
//! They were three copies of the same elimination differing only in how the matrix was stored —
//! `[[f64; N]; N]`, a flat slice, and a `Vec<Vec<f64>>` — which is why storage is the caller's here
//! and this borrows what it is given.

/// Solve `A·x = b` for `x`, overwriting both operands.
///
/// `a` is row-major `n × n` with `n = b.len()`; on return `b` holds `x` and `a` holds the
/// eliminated matrix. `None` when the matrix is singular: a pivot column whose largest remaining
/// magnitude is below `singular_below`, or one that is NaN.
///
/// Destroying the inputs is what lets one routine serve a stack array, a fixed-capacity buffer and
/// a `Vec` alike — a caller that needs `A` again keeps its own copy, which each of the three former
/// implementations was doing anyway to build an augmented matrix. Keeping `b` separate rather than
/// appending it as a column is what makes that copy the caller's whole cost.
///
/// The pivot threshold is a parameter because the callers' matrices are not on one scale: the
/// LM normal matrix is built from pixel fluxes, the distortion ones from coordinates normalized to
/// ~[-1, 1].
#[inline]
pub(crate) fn solve_in_place(a: &mut [f64], b: &mut [f64], singular_below: f64) -> Option<()> {
    let n = b.len();
    debug_assert_eq!(a.len(), n * n, "a must be row-major n×n for n = b.len()");

    for col in 0..n {
        let mut pivot_row = col;
        let mut pivot = a[col * n + col].abs();
        for row in (col + 1)..n {
            let candidate = a[row * n + col].abs();
            if candidate > pivot {
                pivot = candidate;
                pivot_row = row;
            }
        }

        // NaN explicitly, not just `< singular_below`: every ordered comparison against a NaN is
        // false, so a NaN pivot would pass for "large enough" and propagate through the division
        // into a solution of NaNs instead of the documented `None`.
        if pivot.is_nan() || pivot < singular_below {
            return None;
        }

        if pivot_row != col {
            for j in col..n {
                a.swap(col * n + j, pivot_row * n + j);
            }
            b.swap(col, pivot_row);
        }

        // One reciprocal per column rather than a division per row.
        let inv_pivot = 1.0 / a[col * n + col];
        // Split at the row after the pivot's, so the rows being eliminated and the row they read
        // from are separate borrows: the inner update is then a slice-to-slice walk rather than
        // `a[row * n + j]` indexing, which is worth ~20% on an isolated 6×6 solve — a runtime `n`
        // costs the bounds-check elision and unrolling a compile-time one gave for free.
        let (done, remaining) = a.split_at_mut((col + 1) * n);
        let source = &done[col * n + col + 1..col * n + n];
        let (b_done, b_remaining) = b.split_at_mut(col + 1);
        let b_pivot = b_done[col];
        for (row, b_row) in remaining.chunks_exact_mut(n).zip(b_remaining.iter_mut()) {
            let factor = row[col] * inv_pivot;
            // From `col + 1`: the eliminated entry is known to be zero and is never read again.
            for (target, &source) in row[col + 1..].iter_mut().zip(source) {
                *target -= factor * source;
            }
            *b_row -= factor * b_pivot;
        }
    }

    for i in (0..n).rev() {
        let row = &a[i * n..i * n + n];
        let mut sum = b[i];
        for (&coefficient, &x) in row[i + 1..].iter().zip(&b[i + 1..]) {
            sum -= coefficient * x;
        }
        b[i] = sum / row[i];
    }

    Some(())
}

#[cfg(test)]
mod tests;
