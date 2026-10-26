//! [`Lstsq`]: linear least squares by the singular value decomposition.

use nalgebra::{DMatrix, DVector, Dyn, QR, SVD};

/// The cutoff at or below which a singular value of a `rows × cols` design whose largest is
/// `largest` is indistinguishable from zero in f64: `max(rows, cols)·ε·σ_max`.
pub(crate) fn rank_tolerance(rows: usize, cols: usize, largest: f64) -> f64 {
    rows.max(cols) as f64 * f64::EPSILON * largest
}

/// A design matrix factored by its SVD, with its numerical rank decided by one rule: a singular
/// value at or below `max(rows, cols)·ε·σ_max` is indistinguishable from zero in f64, the
/// default cutoff of NumPy's `lstsq` and LAPACK's `gelsd`.
///
/// A design taller than wide is first reduced by a Householder QR, `A = QR`, and its SVD taken of
/// the square `R`, whose singular values are `A`'s: what LAPACK's `gesvd` does for a matrix much
/// taller than wide, both steps backward stable, at a fraction of the work of bidiagonalizing `A`
/// itself and building its left singular vectors.
#[derive(Debug)]
pub(crate) struct Lstsq {
    /// `A`'s Householder reflections, applied to a right-hand side before the SVD solves it; `None`
    /// for a design wider than tall, factored whole for its rank alone.
    qr: Option<QR<f64, Dyn, Dyn>>,
    svd: SVD<f64, Dyn, Dyn>,
    cols: usize,
    tolerance: f64,
}

impl Lstsq {
    pub(crate) fn new(design: DMatrix<f64>) -> Self {
        let (rows, cols) = design.shape();
        let (qr, svd) = if rows >= cols {
            let qr = design.qr();
            let svd = qr.r().svd(true, true);
            (Some(qr), svd)
        } else {
            (None, design.svd(true, true))
        };
        let largest = svd.singular_values.iter().copied().fold(0.0, f64::max);
        Self {
            qr,
            svd,
            cols,
            tolerance: rank_tolerance(rows, cols, largest),
        }
    }

    /// The singular values above the cutoff.
    pub(crate) fn rank(&self) -> usize {
        self.svd.rank(self.tolerance)
    }

    /// The unit vector `x` that minimizes `|A·x|`, up to sign: the right singular vector of the
    /// smallest singular value — the null vector of a design of rank `cols − 1`, and the
    /// total-least-squares one of a full-rank design. `None` when two or more singular values are
    /// at or below the cutoff, so no one vector is the minimizer, and always for a design wider
    /// than tall, whose thin SVD holds none of its null space.
    pub(crate) fn null_vector(&self) -> Option<DVector<f64>> {
        if self.qr.is_none() || self.rank() + 1 < self.cols {
            return None;
        }
        let smallest = self.svd.singular_values.argmin().0;
        let v_t = self
            .svd
            .v_t
            .as_ref()
            .expect("the SVD is computed with its right singular vectors");
        Some(v_t.row(smallest).transpose())
    }

    /// The least-squares solution for each column of `rhs`; `None` when the design is rank
    /// deficient, so the solution is not unique — always for a design wider than tall, which has at
    /// most its row count of independent columns.
    pub(crate) fn solve(&self, rhs: &DMatrix<f64>) -> Option<DMatrix<f64>> {
        let qr = self.qr.as_ref()?;
        if self.rank() != self.cols {
            return None;
        }
        let mut reflected = rhs.clone();
        qr.q_tr_mul(&mut reflected);
        let solution = self
            .svd
            .solve(&reflected.rows(0, self.cols), self.tolerance);
        Some(solution.expect("an SVD computed with both singular-vector sets can solve"))
    }
}

#[cfg(feature = "internals")]
pub(crate) mod bench;

#[cfg(test)]
mod tests {
    use nalgebra::DMatrix;

    use crate::math::lstsq::Lstsq;

    /// The line through (0, 1), (1, 3), (2, 5), (3, 7) is y = 1 + 2x, exactly: a full-rank design
    /// solves to it to rounding. A second column equal to the first gives rank 1 of 2, no
    /// solution, and the null vector `±(1, −1)/√2`; a design wider than tall has at most its row
    /// count, and no null vector its thin SVD could give: `(2, −1)/√5` of `[1 2]` is not in it.
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
        let null = repeated.null_vector().unwrap();
        let unit = std::f64::consts::FRAC_1_SQRT_2;
        assert!((null[0].abs() - unit).abs() < 1e-15 && (null[0] + null[1]).abs() < 1e-15);

        let wide = Lstsq::new(DMatrix::from_row_slice(1, 2, &[1.0, 2.0]));
        assert_eq!(wide.rank(), 1);
        assert!(
            wide.solve(&DMatrix::from_column_slice(1, 1, &[1.0]))
                .is_none()
        );
        assert_eq!(wide.null_vector(), None);
    }
}
