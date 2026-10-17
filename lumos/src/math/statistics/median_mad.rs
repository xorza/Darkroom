//! [`MedianMad`]: a distribution's median and MAD, measured together.

use serde::{Deserialize, Serialize};

use crate::math::statistics;
use crate::math::statistics::float::Float;

/// A sample's location and spread — what one pass over the values measures.
///
/// The spread is always the raw MAD, in the data's own units; [`Self::sigma`] rescales on
/// demand. Carrying the MAD rather than an already-scaled sigma is what keeps "median and MAD"
/// and "median and sigma" one type instead of two, and applies the Gaussian factor exactly once,
/// where the caller needs Gaussian units. The count is what that factor depends on.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct MedianMad<F = f32> {
    pub(crate) median: F,
    pub(crate) mad: F,
    pub(crate) count: usize,
}

impl<F: Float> MedianMad<F> {
    /// Measure both together, in one pass over `data`, which must not be empty, and leave `data`
    /// holding each value's absolute deviation from the median, in no particular order.
    ///
    /// More efficient than computing separately since the median is needed for the MAD.
    pub(crate) fn of_mut(data: &mut [F]) -> Self {
        debug_assert!(!data.is_empty());

        let median = statistics::median_mut(data);
        statistics::abs_deviation_inplace(data, median);
        let mad = statistics::median_of_magnitudes(data);

        Self {
            median,
            mad,
            count: data.len(),
        }
    }

    /// The MAD rescaled to an unbiased estimate of the standard deviation of the normal
    /// distribution the `count` samples came from.
    #[inline]
    pub(crate) fn sigma(self) -> F {
        statistics::sigma_of_mad(self.mad, self.count)
    }
}
