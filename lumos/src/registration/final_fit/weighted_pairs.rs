//! [`WeightedPairs`]: matched positions with a weight each, and the weighted least-squares fit of
//! each model to them.

use glam::DVec2;
use nalgebra::DMatrix;

use crate::math::dmat3::DMat3;
use crate::math::lstsq::Lstsq;
use crate::registration::distortion::sip::{SipFit, SipPairs, SipPolynomial};
use crate::registration::final_fit::homography_refinement::HomographyRefinement;
use crate::registration::final_fit::{FitModel, SipModel};
use crate::registration::point_normalization::PointNormalization;
use crate::registration::ransac::transforms::{dlt_rows, solve_homogeneous_svd_dynamic};
use crate::registration::transform::{Transform, TransformType, WarpTransform};

/// Matched positions paired by index, each pair with its weight: the inverse of its positional
/// variance, times its robust weight.
#[derive(Debug, Default)]
pub(super) struct WeightedPairs {
    pub(super) reference: Vec<DVec2>,
    pub(super) target: Vec<DVec2>,
    pub(super) weights: Vec<f64>,
}

/// Weighted first and second moments of the pairs about their weighted centroids.
#[derive(Debug, Clone, Copy)]
struct Moments {
    reference_centroid: DVec2,
    target_centroid: DVec2,
    /// `Σ wᵢ·r̃ᵢₐ·t̃ᵢᵦ` for the centred positions, `[xx, xy, yx, yy]`.
    cross: [f64; 4],
    /// `Σ wᵢ·|r̃ᵢ|²`.
    reference_spread: f64,
}

impl WeightedPairs {
    pub(super) fn clear(&mut self) {
        self.reference.clear();
        self.target.clear();
        self.weights.clear();
    }

    pub(super) fn push(&mut self, reference: DVec2, target: DVec2, weight: f64) {
        debug_assert!(
            weight.is_finite() && weight > 0.0,
            "a pair's weight is {weight}"
        );
        self.reference.push(reference);
        self.target.push(target);
        self.weights.push(weight);
    }

    pub(super) const fn len(&self) -> usize {
        self.weights.len()
    }

    /// The warp of `model` minimizing `Σ wᵢ·|W(rᵢ) − tᵢ|²`: the transform alone, or the transform
    /// and its SIP correction at their joint optimum ([`SipPolynomial::fit_with`]). `None` when the
    /// pairs do not determine it.
    pub(super) fn fit_warp(&self, model: FitModel) -> Option<WarpTransform> {
        let Some(SipModel { order, origin }) = model.sip else {
            return self.fit(model.transform).map(WarpTransform::new);
        };
        let pairs = SipPairs {
            reference: &self.reference,
            target: &self.target,
            weights: &self.weights,
        };
        let SipFit { transform, sip } =
            SipPolynomial::fit_with(model.transform, pairs, order, origin)?;
        Some(WarpTransform::with_sip(transform, sip))
    }

    /// The `model` minimizing `Σ wᵢ·|M(rᵢ) − tᵢ|²`, in closed form up to an affine map; `None` when
    /// the pairs do not determine it. A homography takes the weighted algebraic (DLT) solution,
    /// which the final fit refines on the reprojection error.
    pub(super) fn fit(&self, model: TransformType) -> Option<Transform> {
        if self.len() < model.min_points() {
            return None;
        }
        match model {
            TransformType::Translation => Some(self.translation()),
            TransformType::Euclidean => self.procrustes(false),
            TransformType::Similarity => self.procrustes(true),
            TransformType::Affine => self.affine(),
            TransformType::Homography => self.homography(),
        }
    }

    /// The weighted mean displacement.
    fn translation(&self) -> Transform {
        let mut weighted = DVec2::ZERO;
        let mut total = 0.0;
        for ((&r, &t), &w) in self.reference.iter().zip(&self.target).zip(&self.weights) {
            weighted += w * (t - r);
            total += w;
        }
        Transform::translation(weighted / total)
    }

    fn moments(&self) -> Moments {
        let total: f64 = self.weights.iter().sum();
        let centroid = |points: &[DVec2]| {
            points
                .iter()
                .zip(&self.weights)
                .fold(DVec2::ZERO, |sum, (&p, &w)| sum + w * p)
                / total
        };
        let (reference_centroid, target_centroid) =
            (centroid(&self.reference), centroid(&self.target));
        let mut cross = [0.0; 4];
        let mut reference_spread = 0.0;
        for ((&r, &t), &w) in self.reference.iter().zip(&self.target).zip(&self.weights) {
            let (r, t) = (r - reference_centroid, t - target_centroid);
            cross[0] += w * r.x * t.x;
            cross[1] += w * r.x * t.y;
            cross[2] += w * r.y * t.x;
            cross[3] += w * r.y * t.y;
            reference_spread += w * r.length_squared();
        }
        Moments {
            reference_centroid,
            target_centroid,
            cross,
            reference_spread,
        }
    }

    /// Weighted Procrustes: the rotation from the weighted cross-covariance, the scale (when
    /// fitted) from it over the weighted reference spread, and the translation that carries the
    /// weighted centroids onto each other. Exact for isotropic per-pair weights.
    fn procrustes(&self, with_scale: bool) -> Option<Transform> {
        let Moments {
            reference_centroid,
            target_centroid,
            cross: [xx, xy, yx, yy],
            reference_spread,
        } = self.moments();
        let angle = (xy - yx).atan2(xx + yy);
        let (sin, cos) = angle.sin_cos();
        let scale = if with_scale {
            if reference_spread <= 0.0 {
                return None;
            }
            let scale = ((xx + yy) * cos + (xy - yx) * sin) / reference_spread;
            if scale <= 0.0 {
                return None;
            }
            scale
        } else {
            1.0
        };
        let rotated = DVec2::new(
            cos * reference_centroid.x - sin * reference_centroid.y,
            sin * reference_centroid.x + cos * reference_centroid.y,
        );
        let translation = target_centroid - scale * rotated;
        Some(if with_scale {
            Transform::similarity(translation, angle, scale)
        } else {
            Transform::euclidean(translation, angle)
        })
    }

    /// The weighted affine map by [`Lstsq`] on Hartley-normalized coordinates, each row scaled by
    /// `√wᵢ`.
    fn affine(&self) -> Option<Transform> {
        let reference_norm = PointNormalization::hartley(&self.reference);
        let target_norm = PointNormalization::hartley(&self.target);
        let n = self.len();
        let mut design = DMatrix::zeros(n, 3);
        let mut rhs = DMatrix::zeros(n, 2);
        for (row, ((&r, &t), &w)) in self
            .reference
            .iter()
            .zip(&self.target)
            .zip(&self.weights)
            .enumerate()
        {
            let root = w.sqrt();
            let (r, t) = (reference_norm.normalize(r), target_norm.normalize(t));
            design[(row, 0)] = root * r.x;
            design[(row, 1)] = root * r.y;
            design[(row, 2)] = root;
            rhs[(row, 0)] = root * t.x;
            rhs[(row, 1)] = root * t.y;
        }
        let solution = Lstsq::new(design).solve(&rhs)?;
        let normalized = Transform::affine([
            solution[(0, 0)],
            solution[(1, 0)],
            solution[(2, 0)],
            solution[(0, 1)],
            solution[(1, 1)],
            solution[(2, 1)],
        ]);
        let transform = target_norm
            .denormalizing_transform()
            .try_compose(&normalized)?
            .try_compose(&reference_norm.normalizing_transform())?;
        transform.is_valid().then_some(transform)
    }

    /// The weighted DLT — each correspondence's two rows scaled by `√wᵢ`, on Hartley-normalized
    /// coordinates, solved for the null vector by the SVD — refined on the weighted reprojection
    /// error by [`HomographyRefinement`]. A DLT whose last entry vanishes in normalized
    /// coordinates, or a refinement that does not converge, leaves the DLT solution.
    fn homography(&self) -> Option<Transform> {
        let reference_norm = PointNormalization::hartley(&self.reference);
        let target_norm = PointNormalization::hartley(&self.target);
        let reference: Vec<DVec2> = self
            .reference
            .iter()
            .map(|&r| reference_norm.normalize(r))
            .collect();
        let target: Vec<DVec2> = self
            .target
            .iter()
            .map(|&t| target_norm.normalize(t))
            .collect();
        let n = self.len();
        let mut design = DMatrix::zeros((2 * n).max(9), 9);
        for (i, ((&r, &t), &w)) in reference.iter().zip(&target).zip(&self.weights).enumerate() {
            let root = w.sqrt();
            let rows = dlt_rows(r, t);
            for column in 0..9 {
                design[(2 * i, column)] = root * rows[0][column];
                design[(2 * i + 1, column)] = root * rows[1][column];
            }
        }
        let mut h = solve_homogeneous_svd_dynamic(design)?;
        let algebraic = *h.as_array();
        if algebraic[8] != 0.0 {
            let start: [f64; 8] = std::array::from_fn(|i| algebraic[i] / algebraic[8]);
            let refinement = HomographyRefinement {
                reference: &reference,
                target: &target,
                weights: &self.weights,
            };
            if let Some(refined) = refinement.refine(start) {
                h = DMat3::from_array([
                    refined[0], refined[1], refined[2], refined[3], refined[4], refined[5],
                    refined[6], refined[7], 1.0,
                ]);
            }
        }
        let denormalized = DMat3::from_array(*target_norm.denormalizing_transform().matrix())
            .mul_mat(&h)
            .mul_mat(&DMat3::from_array(
                *reference_norm.normalizing_transform().matrix(),
            ));
        Transform::from_homography_matrix(denormalized).filter(Transform::is_valid)
    }
}
