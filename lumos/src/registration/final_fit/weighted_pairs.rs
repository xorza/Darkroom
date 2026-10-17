//! [`WeightedPairs`]: matched positions with a weight each, and the weighted least-squares fit of
//! each model to them.

use glam::DVec2;

use crate::math::dmat3::DMat3;
use crate::registration::distortion::sip::{SipFit, SipPairs, SipPolynomial};
use crate::registration::estimate::Pairs;
use crate::registration::estimate::dlt::Dlt;
use crate::registration::final_fit::FitModel;
use crate::registration::final_fit::homography_refinement::HomographyRefinement;
use crate::registration::transform::{Transform, TransformType};
use crate::registration::warp_transform::WarpTransform;

/// Matched positions paired by index, each pair with its weight: the inverse of its positional
/// variance, times its robust weight.
#[derive(Debug, Default)]
pub(super) struct WeightedPairs {
    pub(super) reference: Vec<DVec2>,
    pub(super) target: Vec<DVec2>,
    pub(super) weights: Vec<f64>,
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
        let (linear, sip) = match model {
            FitModel::Plain(transform_type) => {
                return self.fit(transform_type).map(WarpTransform::new);
            }
            FitModel::WithSip(linear, sip) => (linear, sip),
        };
        let pairs = SipPairs {
            reference: &self.reference,
            target: &self.target,
            weights: &self.weights,
        };
        let SipFit { transform, sip } = SipPolynomial::fit_with(linear, pairs, sip)?;
        Some(WarpTransform::with_sip(transform, sip))
    }

    /// The pairs as the estimators take them.
    fn pairs(&self) -> Pairs<'_> {
        Pairs::weighted(&self.reference, &self.target, &self.weights)
    }

    /// The `model` minimizing `Σ wᵢ·|M(rᵢ) − tᵢ|²` ([`Pairs::estimate`]); `None` when the pairs do
    /// not determine it. A homography's algebraic (DLT) solution is refined on the reprojection
    /// error.
    pub(super) fn fit(&self, model: TransformType) -> Option<Transform> {
        match model {
            TransformType::Homography if self.len() >= model.min_points() => self.homography(),
            _ => self.pairs().estimate(model),
        }
    }

    /// The weighted DLT refined on the weighted reprojection error by [`HomographyRefinement`],
    /// in the DLT's normalized coordinates. A DLT whose last entry vanishes there, or a refinement
    /// that does not converge, leaves the DLT solution.
    fn homography(&self) -> Option<Transform> {
        let pairs = self.pairs();
        let dlt = Dlt::of(pairs);
        let mut h = dlt.solve(pairs)?;
        let algebraic = *h.as_array();
        if algebraic[8] != 0.0 {
            let reference: Vec<DVec2> = self
                .reference
                .iter()
                .map(|&r| dlt.reference.normalize(r))
                .collect();
            let target: Vec<DVec2> = self
                .target
                .iter()
                .map(|&t| dlt.target.normalize(t))
                .collect();
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
        dlt.denormalize(h)
    }
}
