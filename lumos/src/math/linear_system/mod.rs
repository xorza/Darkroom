//! [`Lu`]: a small dense matrix factored by Gaussian elimination with partial pivoting, and solved
//! for one right-hand side at a time.

/// A row-major `n × n` matrix factored as `P·A = L·U` in the caller's storage: `U` on and above the
/// diagonal, `L`'s multipliers below it — its unit diagonal implied — and `P` as the row each step
/// swapped in. Storage is the caller's, so a stack array and a `Vec` serve alike, and a caller that
/// needs `A` again keeps its own copy.
#[derive(Debug)]
pub(crate) struct Lu<'a> {
    factors: &'a [f64],
    pivots: &'a [usize],
}

impl<'a> Lu<'a> {
    /// `a` factored in place, `pivots` holding one row index per column; `None` when the matrix is
    /// singular: a pivot column whose largest remaining magnitude is below `singular_below`, or
    /// one that is NaN.
    ///
    /// The threshold is the caller's, as the matrices are on the scales their callers build them
    /// at.
    pub(crate) fn factor(
        a: &'a mut [f64],
        pivots: &'a mut [usize],
        singular_below: f64,
    ) -> Option<Self> {
        let n = pivots.len();
        debug_assert_eq!(a.len(), n * n, "a must be row-major n×n for n pivots");
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
            // NaN explicitly, not just `< singular_below`: every ordered comparison against a NaN
            // is false, so a NaN pivot would pass for "large enough" and propagate through the
            // division into a solution of NaNs instead of the documented `None`.
            if pivot.is_nan() || pivot < singular_below {
                return None;
            }
            pivots[col] = pivot_row;
            if pivot_row != col {
                for j in 0..n {
                    a.swap(col * n + j, pivot_row * n + j);
                }
            }
            let inv_pivot = 1.0 / a[col * n + col];
            // Split at the row after the pivot's, so the rows being eliminated and the row they
            // read from are separate borrows: the inner update is then a slice-to-slice walk.
            let (done, remaining) = a.split_at_mut((col + 1) * n);
            let source = &done[col * n + col + 1..col * n + n];
            for row in remaining.chunks_exact_mut(n) {
                let factor = row[col] * inv_pivot;
                row[col] = factor;
                for (target, &source) in row[col + 1..].iter_mut().zip(source) {
                    *target -= factor * source;
                }
            }
        }
        Some(Self { factors: a, pivots })
    }

    /// Solve `A·x = b`, `b` overwritten by `x`: the row swaps, then `L` forward and `U` back.
    pub(crate) fn solve(&self, b: &mut [f64]) {
        let n = self.pivots.len();
        debug_assert_eq!(b.len(), n);
        for (col, &pivot_row) in self.pivots.iter().enumerate() {
            b.swap(col, pivot_row);
        }
        for i in 1..n {
            let row = &self.factors[i * n..i * n + i];
            let mut sum = b[i];
            for (&multiplier, &x) in row.iter().zip(&b[..i]) {
                sum -= multiplier * x;
            }
            b[i] = sum;
        }
        for i in (0..n).rev() {
            let row = &self.factors[i * n..i * n + n];
            let mut sum = b[i];
            for (&coefficient, &x) in row[i + 1..].iter().zip(&b[i + 1..]) {
                sum -= coefficient * x;
            }
            b[i] = sum / row[i];
        }
    }
}

#[cfg(feature = "internals")]
pub(crate) mod bench;

#[cfg(test)]
mod tests;
