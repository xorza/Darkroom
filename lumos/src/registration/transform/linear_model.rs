//! [`LinearModel`]: a transform model a SIP correction can sit under.

use crate::registration::transform::TransformType;

/// A [`TransformType`] up to affine: the linear map a SIP correction sits under. A homography's
/// perspective terms are the correction's quadratic ones, so the two are not determined together
/// — which is why the SIP convention puts an affine map under the polynomial.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinearModel {
    Translation,
    Euclidean,
    Similarity,
    Affine,
}

impl LinearModel {
    /// The models `Auto` chooses among under a SIP correction, from the fewest degrees of freedom
    /// to the most.
    pub(crate) const AUTO: [Self; 3] = [Self::Euclidean, Self::Similarity, Self::Affine];
}

impl From<LinearModel> for TransformType {
    fn from(model: LinearModel) -> Self {
        match model {
            LinearModel::Translation => Self::Translation,
            LinearModel::Euclidean => Self::Euclidean,
            LinearModel::Similarity => Self::Similarity,
            LinearModel::Affine => Self::Affine,
        }
    }
}

impl TryFrom<TransformType> for LinearModel {
    /// A homography, which no SIP correction sits under.
    type Error = TransformType;

    fn try_from(model: TransformType) -> Result<Self, TransformType> {
        match model {
            TransformType::Translation => Ok(Self::Translation),
            TransformType::Euclidean => Ok(Self::Euclidean),
            TransformType::Similarity => Ok(Self::Similarity),
            TransformType::Affine => Ok(Self::Affine),
            TransformType::Homography => Err(model),
        }
    }
}
