//! [`Dlt`]: the direct linear transform, a homography's algebraic solution.

use glam::DVec2;
use nalgebra::{Dim, Dyn, Matrix, OMatrix, RawStorageMut, SMatrix, SVD, U9};

use crate::math::dmat3::DMat3;
use crate::registration::estimate::{Pair, Pairs};
use crate::registration::point_normalization::PointNormalization;
use crate::registration::transform::Transform;

/// The direct linear transform of a set of pairs: the homography `h` whose rows satisfy
/// `A·h = 0` best in the weighted algebraic sense, solved on Hartley-normalized coordinates.
///
/// Degenerate when the SVD finds no null vector or the denormalized matrix is not an invertible
/// homography; a solve on points nearly in a degenerate configuration gives a matrix the RANSAC
/// plausibility gate and the final fit's reprojection error judge, not this.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Dlt {
    pub(crate) reference: PointNormalization,
    pub(crate) target: PointNormalization,
}

impl Dlt {
    /// The normalizations of `pairs`: Hartley's, for each side.
    pub(crate) fn of(pairs: Pairs<'_>) -> Self {
        Self {
            reference: PointNormalization::hartley(pairs.reference),
            target: PointNormalization::hartley(pairs.target),
        }
    }

    /// The unit-norm null vector of the system each pair adds two rows to, each scaled by `√wᵢ`, in
    /// normalized coordinates. A minimal set fills a fixed 9 × 9 matrix, its last row zero, and
    /// allocates nothing; more pairs keep the whole rectangular system, whose SVD conditions as `κ`
    /// where the normal equations `AᵀA` would as `κ²`.
    pub(crate) fn solve(self, pairs: Pairs<'_>) -> Option<DMat3> {
        if pairs.len() * 2 <= 9 {
            let mut design = SMatrix::<f64, 9, 9>::zeros();
            self.fill(&mut design, pairs);
            Some(null_vector(
                SVD::new(design, false, true).v_t?.row(8).iter(),
            ))
        } else {
            let mut design = OMatrix::<f64, Dyn, U9>::zeros(2 * pairs.len());
            self.fill(&mut design, pairs);
            Some(null_vector(
                SVD::new(design, false, true).v_t?.row(8).iter(),
            ))
        }
    }

    /// `h`, solved in normalized coordinates, as a homography of the pairs' own:
    /// `H = D_target · h · N_reference`. The null vector has unit norm and no particular last
    /// entry, so the product is normalized once, at the end.
    pub(crate) fn denormalize(self, h: DMat3) -> Option<Transform> {
        let denormalized = DMat3::from_array(*self.target.denormalizing_transform().matrix())
            .mul_mat(&h)
            .mul_mat(&DMat3::from_array(
                *self.reference.normalizing_transform().matrix(),
            ));
        Transform::from_homography_matrix(denormalized).filter(Transform::is_valid)
    }

    /// The rows of the DLT system, two a pair, each scaled by `√wᵢ`:
    /// `[−x −y −1 0 0 0 x·x′ y·x′ x′]` and `[0 0 0 −x −y −1 x·y′ y·y′ y′]`.
    fn fill<R: Dim, S: RawStorageMut<f64, R, U9>>(
        self,
        design: &mut Matrix<f64, R, U9, S>,
        pairs: Pairs<'_>,
    ) {
        for (
            i,
            Pair {
                reference,
                target,
                weight,
            },
        ) in pairs.iter().enumerate()
        {
            let root = weight.sqrt();
            let DVec2 { x, y } = self.reference.normalize(reference);
            let t = self.target.normalize(target);
            let rows = [
                [-x, -y, -1.0, 0.0, 0.0, 0.0, x * t.x, y * t.x, t.x],
                [0.0, 0.0, 0.0, -x, -y, -1.0, x * t.y, y * t.y, t.y],
            ];
            for (offset, row) in rows.iter().enumerate() {
                for (column, &value) in row.iter().enumerate() {
                    design[(2 * i + offset, column)] = root * value;
                }
            }
        }
    }
}

/// The last right singular vector, the system's null vector, as a 3 × 3 matrix.
fn null_vector<'a>(row: impl Iterator<Item = &'a f64>) -> DMat3 {
    let mut data = [0.0; 9];
    for (entry, &value) in data.iter_mut().zip(row) {
        *entry = value;
    }
    DMat3::from_array(data)
}
