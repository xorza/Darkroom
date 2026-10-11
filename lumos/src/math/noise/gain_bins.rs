//! [`GainBins`]: the bins of flat gain a frame's noise is measured over.

use arrayvec::ArrayVec;
use serde::{Deserialize, Serialize};

/// Bins of equal count a frame's noise is measured over, by flat gain: enough to fit two terms
/// with residuals to spare, each holding an eighth of the samples.
pub(crate) const GAIN_BINS: usize = 8;

const _: () = assert!(GAIN_BINS <= 1 << u8::BITS, "a sample's bin is a byte");

/// The bounds between [`GAIN_BINS`] bins of equal count over a frame's flat gain, ascending.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct GainBins {
    bounds: ArrayVec<f32, { GAIN_BINS - 1 }>,
}

impl GainBins {
    /// The bounds of equal-count bins over `gains`, which this sorts.
    pub(crate) fn of(gains: &mut [f32]) -> Self {
        gains.sort_unstable_by(f32::total_cmp);
        Self {
            bounds: (1..GAIN_BINS)
                .map(|bin| gains[(gains.len() * bin / GAIN_BINS).min(gains.len() - 1)])
                .collect(),
        }
    }

    /// The bin `gain` falls in.
    pub(crate) fn bin(&self, gain: f32) -> usize {
        self.bounds.partition_point(|&bound| bound <= gain)
    }
}

#[cfg(test)]
mod tests {
    use crate::math::noise::gain_bins::GainBins;

    /// Sixteen gains 1 to 16 cut into eight bins of two: the bounds are the gains at ranks 2, 4,
    /// …, 14, so 3, 5, …, 15, and a gain falls in the bin whose bound it has not reached.
    #[test]
    fn gain_bins_hold_equal_counts() {
        let mut gains: Vec<f32> = (1..=16).rev().map(|gain| gain as f32).collect();
        let bins = GainBins::of(&mut gains);
        for (gain, bin) in [
            (1.0, 0),
            (2.0, 0),
            (3.0, 1),
            (14.0, 6),
            (15.0, 7),
            (16.0, 7),
        ] {
            assert_eq!(bins.bin(gain), bin, "gain {gain}");
        }
    }
}
