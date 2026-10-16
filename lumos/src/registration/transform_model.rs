//! [`TransformModel`]: which model a registration fits.

use crate::registration::distortion::sip::SipConfig;
use crate::registration::transform::TransformType;
use crate::registration::transform::linear_model::LinearModel;

/// Which model a registration should fit: a specific one, or a request to choose.
///
/// Kept apart from [`TransformType`] so that "pick a model for me" cannot reach the places that
/// can only act on a chosen one — RANSAC, transform estimation, and [`Transform`](crate::Transform) itself, each of
/// which would otherwise need its own arm rejecting it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TransformModel {
    /// Fit exactly this model, with no distortion correction.
    Fixed(TransformType),
    /// Fit exactly this model, up to affine, under a SIP distortion correction.
    FixedWithSip(LinearModel, SipConfig),
    /// Fit every model from Euclidean to Homography — to Affine under a SIP correction — and take
    /// the one of lowest geometric robust information criterion (Torr 1998): the one whose
    /// residuals, over every pair any model matched, best repay its degrees of freedom.
    Auto { sip: Option<SipConfig> },
}

impl Default for TransformModel {
    fn default() -> Self {
        Self::Auto { sip: None }
    }
}

impl TransformModel {
    /// The most general model this could resolve to — `Auto`'s ceiling, or the fixed choice.
    ///
    /// What the star-count and match-count gates size against, since a run that may climb to
    /// homography has to arrive with enough points to fit one.
    pub const fn most_general(self) -> TransformType {
        match self {
            Self::Fixed(transform_type) => transform_type,
            Self::FixedWithSip(LinearModel::Translation, _) => TransformType::Translation,
            Self::FixedWithSip(LinearModel::Euclidean, _) => TransformType::Euclidean,
            Self::FixedWithSip(LinearModel::Similarity, _) => TransformType::Similarity,
            Self::FixedWithSip(LinearModel::Affine, _) | Self::Auto { sip: Some(_) } => {
                TransformType::Affine
            }
            Self::Auto { sip: None } => TransformType::Homography,
        }
    }

    /// The SIP correction this model fits, if any.
    pub const fn sip(&self) -> Option<&SipConfig> {
        match self {
            Self::FixedWithSip(_, sip) | Self::Auto { sip: Some(sip) } => Some(sip),
            Self::Fixed(_) | Self::Auto { sip: None } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::registration::transform::TransformType;
    use crate::registration::transform_model::TransformModel;

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
                TransformModel::Fixed(transform_type).most_general(),
                transform_type
            );
        }
    }
}
