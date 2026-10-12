//! [`MedianMad`]: a distribution's median and MAD, measured together.

use serde::{Deserialize, Serialize};

use crate::math::statistics::{
    abs_deviation_inplace, mad_to_sigma, median_mut, median_of_magnitudes,
};

/// A distribution's location and spread — what one pass over the values measures.
///
/// The spread is always the raw MAD, in the data's own units; [`Self::sigma`] rescales on
/// demand. Carrying the MAD rather than an already-scaled sigma is what keeps "median and MAD"
/// and "median and sigma" one type instead of two, and applies the 1.4826 factor exactly once,
/// where the caller needs Gaussian units.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct MedianMad {
    pub(crate) median: f32,
    pub(crate) mad: f32,
}

impl MedianMad {
    /// Measure both together, in one pass over `data`.
    ///
    /// More efficient than computing separately since the median is needed for the MAD.
    /// Mutates the input buffer.
    pub(crate) fn of_mut(data: &mut [f32]) -> Self {
        debug_assert!(!data.is_empty());

        let median = median_mut(data);
        abs_deviation_inplace(data, median);
        let mad = median_of_magnitudes(data);

        Self { median, mad }
    }

    /// The MAD rescaled to the standard deviation of an equivalent normal distribution.
    #[inline]
    pub(crate) const fn sigma(self) -> f32 {
        mad_to_sigma(self.mad)
    }
}
