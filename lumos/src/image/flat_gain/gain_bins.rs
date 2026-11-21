//! [`GainBins`]: the bins of flat gain a frame's noise is measured over.

use std::array;

use serde::{Deserialize, Serialize};

/// Bins of equal count a frame's noise is measured over, by flat gain: enough to fit two terms
/// with residuals to spare, each holding an eighth of the samples.
pub(crate) const GAIN_BINS: usize = 8;

const _: () = assert!(GAIN_BINS <= 1 << u8::BITS, "a sample's bin is a byte");

/// The bounds between [`GAIN_BINS`] bins of equal count over a frame's flat gain, ascending.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct GainBins {
    bounds: [f32; GAIN_BINS - 1],
}

impl GainBins {
    /// The bounds of equal-count bins over `gains`, which this sorts.
    pub(crate) fn of(gains: &mut [f32]) -> Self {
        debug_assert!(!gains.is_empty(), "a gain plane holds a node at least");
        // The ranks ascend, so each selection runs over the part past the last: everything before
        // a selected rank is no greater than it, and the next rank's value lies after it.
        let mut start = 0;
        Self {
            bounds: array::from_fn(|bound| {
                let rank = (gains.len() * (bound + 1) / GAIN_BINS).min(gains.len() - 1);
                let (_, &mut value, _) =
                    gains[start..].select_nth_unstable_by(rank - start, f32::total_cmp);
                start = rank;
                value
            }),
        }
    }

    /// The bin `gain` falls in: how many bounds it has reached. The bounds ascend, so this is their
    /// partition point, counted without a branch per bound.
    pub(crate) fn bin(&self, gain: f32) -> usize {
        self.bounds
            .iter()
            .map(|&bound| usize::from(bound <= gain))
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use crate::image::flat_gain::gain_bins::{GAIN_BINS, GainBins};

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

    /// The selected bounds are the sorted gains at their ranks, bit for bit, over a plane of
    /// repeated and scattered values whose ranks fall on runs of equal gains, and one of three.
    #[test]
    fn selected_bounds_are_the_sorted_ranks() {
        for len in [3_usize, 7, 1000] {
            let gains: Vec<f32> = (0..len)
                .map(|idx| ((idx * 7919) % 97) as f32 / 8.0)
                .collect();
            let mut sorted = gains.clone();
            sorted.sort_unstable_by(f32::total_cmp);
            let mut selected = gains;
            let bins = GainBins::of(&mut selected);
            for (bound, value) in bins.bounds.iter().enumerate() {
                let rank = (len * (bound + 1) / GAIN_BINS).min(len - 1);
                assert_eq!(
                    value.to_bits(),
                    sorted[rank].to_bits(),
                    "{len}: bound {bound}"
                );
            }
        }
    }
}
