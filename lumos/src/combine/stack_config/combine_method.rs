//! [`CombineMethod`]: how the samples at a pixel combine.

use crate::combine::rejection::Rejection;
use crate::stack_product::quality_planes::QualityPlanes;

/// Method for combining pixel values across frames.
///
/// Rejection is only available with `Mean` — median is already robust to outliers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CombineMethod {
    /// Mean value with optional rejection. If weights are provided, computes weighted mean.
    Mean(Rejection),
    /// Median value (implicit outlier rejection, no explicit rejection needed).
    Median,
}

impl CombineMethod {
    /// The planes of `requested` this method can produce, so the request reaching the reducer is
    /// exactly what it will write: inverse variance and dispersion belong to a weighted mean, and a
    /// median is not a linear combination.
    pub(crate) const fn planes(self, requested: QualityPlanes) -> QualityPlanes {
        let weighted_mean = matches!(self, Self::Mean(_));
        QualityPlanes {
            inverse_variance: requested.inverse_variance && weighted_mean,
            dispersion: requested.dispersion && weighted_mean,
            ..requested
        }
    }
}
