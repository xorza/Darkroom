//! Each transform model fitted to matched positions by weighted least squares: one estimator per
//! model, the same for a RANSAC sample, its local refits and the final fit.
//!
//! A model's points are degenerate when its fit keeps fewer than half the digits of `f64`: the
//! map is then set by rounding as much as by the points. Each model states what that means for
//! its own solve.

pub(crate) mod dlt;
mod moments;

use glam::DVec2;

use crate::registration::estimate::dlt::Dlt;
use crate::registration::estimate::moments::{Moments, Scale};
use crate::registration::transform::{Transform, TransformType};

/// Matched positions paired by index, each pair with its weight.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Pairs<'a> {
    reference: &'a [DVec2],
    target: &'a [DVec2],
    weights: Weights<'a>,
}

/// The pairs' weights: every pair at 1, which a RANSAC sample takes without allocating them, or
/// one each.
#[derive(Debug, Clone, Copy)]
enum Weights<'a> {
    Unit,
    Each(&'a [f64]),
}

/// One weighted pair.
#[derive(Debug, Clone, Copy)]
struct Pair {
    reference: DVec2,
    target: DVec2,
    weight: f64,
}

impl<'a> Pairs<'a> {
    /// `reference` and `target` paired by index, every pair at weight 1.
    pub(crate) fn unit(reference: &'a [DVec2], target: &'a [DVec2]) -> Self {
        debug_assert_eq!(reference.len(), target.len());
        Self {
            reference,
            target,
            weights: Weights::Unit,
        }
    }

    /// `reference` and `target` paired by index, each pair at its finite, positive weight.
    pub(crate) fn weighted(
        reference: &'a [DVec2],
        target: &'a [DVec2],
        weights: &'a [f64],
    ) -> Self {
        debug_assert_eq!(reference.len(), target.len());
        debug_assert_eq!(reference.len(), weights.len());
        debug_assert!(
            weights.iter().all(|&w| w.is_finite() && w > 0.0),
            "a pair's weight is finite and positive"
        );
        Self {
            reference,
            target,
            weights: Weights::Each(weights),
        }
    }

    pub(crate) const fn len(self) -> usize {
        self.reference.len()
    }

    fn iter(self) -> impl Iterator<Item = Pair> + 'a {
        let weights = self.weights;
        self.reference.iter().zip(self.target).enumerate().map(
            move |(idx, (&reference, &target))| Pair {
                reference,
                target,
                weight: match weights {
                    Weights::Unit => 1.0,
                    Weights::Each(weights) => weights[idx],
                },
            },
        )
    }

    /// The `model` minimizing `Σ wᵢ·|M(rᵢ) − tᵢ|²` in closed form up to an affine map, and a
    /// homography's algebraic (DLT) solution; `None` with fewer pairs than the model's minimal set,
    /// or when they are degenerate for it.
    pub(crate) fn estimate(self, model: TransformType) -> Option<Transform> {
        if self.len() < model.min_points() {
            return None;
        }
        match model {
            TransformType::Translation => Some(self.translation()),
            TransformType::Euclidean => Moments::of(self).procrustes(Scale::Unit),
            TransformType::Similarity => Moments::of(self).procrustes(Scale::Fitted),
            TransformType::Affine => Moments::of(self).affine(),
            TransformType::Homography => {
                let dlt = Dlt::of(self);
                dlt.denormalize(dlt.solve(self)?)
            }
        }
    }

    /// The weighted mean displacement, which no set of pairs leaves undetermined.
    fn translation(self) -> Transform {
        let mut weighted = DVec2::ZERO;
        let mut total = 0.0;
        for Pair {
            reference,
            target,
            weight,
        } in self.iter()
        {
            weighted += weight * (target - reference);
            total += weight;
        }
        Transform::translation(weighted / total)
    }
}

#[cfg(test)]
mod tests;
