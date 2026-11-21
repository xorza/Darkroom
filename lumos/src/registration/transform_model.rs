//! [`TransformModel`]: which model a registration fits.

use crate::registration::distortion::sip::SipConfig;
use crate::registration::transform::TransformType;
use crate::registration::transform::linear_model::LinearModel;

/// A model fixed by the caller, or the one registration chooses.
///
/// Kept apart from [`TransformType`] so that "pick a model for me" cannot reach the places that can
/// only act on a chosen one — RANSAC, transform estimation, and [`Transform`](crate::Transform)
/// itself, each of which would otherwise need its own arm rejecting it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ModelChoice<M> {
    /// Fit exactly this model.
    Fixed(M),
    /// Fit every model the choice spans and take the one of lowest geometric robust information
    /// criterion (Torr 1998): the one whose residuals, over every pair any model matched, best
    /// repay its degrees of freedom.
    #[default]
    Auto,
}

/// Which model a registration fits: a linear transform alone, or one under a SIP distortion
/// correction, which fits up to affine.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TransformModel {
    /// No distortion correction: `Auto` chooses from Euclidean to Homography.
    Plain(ModelChoice<TransformType>),
    /// A SIP correction under the linear model: `Auto` chooses from Euclidean to Affine, each with
    /// the correction and without, so a frame too sparse for the correction registers by a plain
    /// model.
    WithSip(ModelChoice<LinearModel>, SipConfig),
}

impl Default for TransformModel {
    fn default() -> Self {
        Self::Plain(ModelChoice::Auto)
    }
}

impl TransformModel {
    /// The most general model this could resolve to — `Auto`'s ceiling, or the fixed choice.
    ///
    /// What the star-count and match-count gates size against, since a run that may climb to
    /// homography has to arrive with enough points to fit one.
    pub const fn most_general(self) -> TransformType {
        match self {
            Self::Plain(ModelChoice::Fixed(transform_type)) => transform_type,
            Self::Plain(ModelChoice::Auto) => TransformType::Homography,
            Self::WithSip(ModelChoice::Fixed(LinearModel::Translation), _) => {
                TransformType::Translation
            }
            Self::WithSip(ModelChoice::Fixed(LinearModel::Euclidean), _) => {
                TransformType::Euclidean
            }
            Self::WithSip(ModelChoice::Fixed(LinearModel::Similarity), _) => {
                TransformType::Similarity
            }
            Self::WithSip(ModelChoice::Fixed(LinearModel::Affine) | ModelChoice::Auto, _) => {
                TransformType::Affine
            }
        }
    }

    /// The stars each catalog needs: twice the minimal sample of the most general model this
    /// could resolve to, floored at three for triangle matching. `Auto` can climb to homography
    /// and must arrive with enough stars to fit one.
    pub const fn required_stars(self) -> usize {
        let required = 2 * self.most_general().min_points();
        if required > 3 { required } else { 3 }
    }

    /// The SIP correction this model fits, if any.
    pub const fn sip(&self) -> Option<&SipConfig> {
        match self {
            Self::WithSip(_, sip) => Some(sip),
            Self::Plain(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::registration::transform::TransformType;
    use crate::registration::transform_model::{ModelChoice, TransformModel};

    #[test]
    fn auto_sizes_its_gates_against_the_most_general_model_it_fits() {
        // `Auto` fits up to Homography, so every count it is measured by has to be Homography's —
        // anything smaller would let a pair through that the most general model cannot fit.
        assert_eq!(
            TransformModel::default().most_general(),
            TransformType::Homography
        );
        assert_eq!(TransformModel::default().most_general().min_points(), 4);
        // A fixed choice is its own ceiling.
        for transform_type in [
            TransformType::Translation,
            TransformType::Euclidean,
            TransformType::Similarity,
            TransformType::Affine,
            TransformType::Homography,
        ] {
            assert_eq!(
                TransformModel::Plain(ModelChoice::Fixed(transform_type)).most_general(),
                transform_type
            );
        }
    }
}
