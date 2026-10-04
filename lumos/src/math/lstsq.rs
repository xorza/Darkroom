//! [`Lstsq`]: linear least squares by the singular value decomposition.

use nalgebra::{DMatrix, Dyn, SVD};

/// A design matrix factored by its SVD, with its numerical rank decided by one rule: a singular
/// value at or below `max(rows, columns)·ε·σ_max` is indistinguishable from zero in f64, the
/// default cutoff of NumPy's `lstsq` and LAPACK's `gelsd`.
#[derive(Debug)]
pub(crate) struct Lstsq {
    svd: SVD<f64, Dyn, Dyn>,
    columns: usize,
    tolerance: f64,
}

impl Lstsq {
    pub(crate) fn new(design: DMatrix<f64>) -> Self {
        let (rows, columns) = design.shape();
        let svd = design.svd(true, true);
        let largest = svd.singular_values.iter().copied().fold(0.0, f64::max);
        Self {
            svd,
            columns,
            tolerance: rows.max(columns) as f64 * f64::EPSILON * largest,
        }
    }

    /// The singular values above the cutoff.
    pub(crate) fn rank(&self) -> usize {
        self.svd.rank(self.tolerance)
    }

    /// The least-squares solution for each column of `rhs`; `None` when the design is rank
    /// deficient, so the solution is not unique.
    pub(crate) fn solve(&self, rhs: &DMatrix<f64>) -> Option<DMatrix<f64>> {
        (self.rank() == self.columns).then(|| {
            self.svd
                .solve(rhs, self.tolerance)
                .expect("an SVD computed with both singular-vector sets can solve")
        })
    }
}

#[cfg(test)]
mod tests {
    use nalgebra::DMatrix;

    use crate::math::lstsq::Lstsq;

    /// The line through (0, 1), (1, 3), (2, 5), (3, 7) is y = 1 + 2x, exactly: a full-rank design
    /// solves to it to rounding. A second column equal to the first gives rank 1 of 2, and no
    /// solution; a design wider than tall has at most its row count.
    #[test]
    fn full_rank_solves_and_deficient_does_not() {
        let design = DMatrix::from_row_slice(4, 2, &[1.0, 0.0, 1.0, 1.0, 1.0, 2.0, 1.0, 3.0]);
        let rhs = DMatrix::from_column_slice(4, 1, &[1.0, 3.0, 5.0, 7.0]);
        let lstsq = Lstsq::new(design);
        assert_eq!(lstsq.rank(), 2);
        let solution = lstsq.solve(&rhs).unwrap();
        assert!((solution[(0, 0)] - 1.0).abs() < 1e-14);
        assert!((solution[(1, 0)] - 2.0).abs() < 1e-14);

        let repeated = Lstsq::new(DMatrix::from_row_slice(
            3,
            2,
            &[1.0, 1.0, 2.0, 2.0, 3.0, 3.0],
        ));
        assert_eq!(repeated.rank(), 1);
        assert!(
            repeated
                .solve(&DMatrix::from_column_slice(3, 1, &[1.0, 2.0, 3.0]))
                .is_none()
        );

        let wide = Lstsq::new(DMatrix::from_row_slice(1, 2, &[1.0, 2.0]));
        assert_eq!(wide.rank(), 1);
        assert!(
            wide.solve(&DMatrix::from_column_slice(1, 1, &[1.0]))
                .is_none()
        );
    }
}
