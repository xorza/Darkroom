//! [`SortedSamples`]: one pixel's samples in ascending order, each with the gather position it came
//! from.

use std::ops::Range;

use crate::math::statistics::float;

/// A pixel's samples sorted once, for every rejection method to narrow a window of.
///
/// The sort runs on `u64` keys: the sample's bits mapped to an unsigned integer of the same order in
/// the high half, its gather position in the low half. One integer compare and one move per step,
/// where sorting values and positions as two arrays swaps both, and equal values come out in gather
/// order, so the order is deterministic.
#[derive(Debug, Default)]
pub(crate) struct SortedSamples {
    keys: Vec<u64>,
    values: Vec<f32>,
    poss: Vec<u32>,
    /// Whether each sample of the window [`Self::classify`] last saw is kept, from its start.
    kept: Vec<bool>,
}

impl SortedSamples {
    /// Emptied, with room for `count` samples, so the per-pixel refill never allocates.
    pub(crate) fn reserve(&mut self, count: usize) {
        let Self {
            keys,
            values,
            poss,
            kept,
        } = self;
        keys.clear();
        keys.reserve_exact(count);
        values.clear();
        values.reserve_exact(count);
        poss.clear();
        poss.reserve_exact(count);
        kept.clear();
        kept.reserve_exact(count);
    }

    /// The bytes [`Self::reserve`] holds for `count` samples.
    pub(crate) const fn heap_bytes(count: usize) -> usize {
        count.saturating_mul(
            size_of::<u64>() + size_of::<f32>() + size_of::<u32>() + size_of::<bool>(),
        )
    }

    /// Sort `values`, which must be finite, remembering where each came from.
    pub(crate) fn fill(&mut self, values: &[f32]) {
        debug_assert!(values.iter().all(|value| value.is_finite()));
        debug_assert!(u32::try_from(values.len()).is_ok());
        self.keys.clear();
        self.keys.extend(
            values
                .iter()
                .enumerate()
                .map(|(pos, &value)| (u64::from(float::unsigned_key(value)) << 32) | pos as u64),
        );
        self.keys.sort_unstable();
        self.values.clear();
        self.poss.clear();
        for &key in &self.keys {
            let pos = key as u32;
            self.values.push(values[pos as usize]);
            self.poss.push(pos);
        }
    }

    pub(crate) fn values(&self) -> &[f32] {
        &self.values
    }

    /// Each sorted sample's position in the gather it came from.
    pub(crate) fn poss(&self) -> &[u32] {
        &self.poss
    }

    /// Decide once, by `keep` on each one's value and gather position, which samples of `window`
    /// stay, for [`Self::partition`] to move; how many stay.
    pub(crate) fn classify(
        &mut self,
        window: Range<usize>,
        keep: impl Fn(f32, u32) -> bool,
    ) -> usize {
        self.kept.clear();
        self.kept
            .extend(window.map(|idx| keep(self.values[idx], self.poss[idx])));
        self.kept.iter().filter(|&&kept| kept).count()
    }

    /// Move the samples of `window` that [`Self::classify`] rejected to its ends — those below
    /// `centre` to its start, the rest to its end — and return the run of kept ones between them,
    /// still ascending, so a later pass reads them as a sorted window. The rejected ones keep no
    /// order: nothing reads them again. The sort keys serve as the scratch, so nothing allocates.
    pub(crate) fn partition(&mut self, window: Range<usize>, centre: f32) -> Range<usize> {
        debug_assert_eq!(self.kept.len(), window.len(), "the window classified");
        let pack = |value: f32, pos: u32| (u64::from(value.to_bits()) << 32) | u64::from(pos);
        let side = |idx: usize, kept: bool| {
            let value = self.values[idx];
            match (kept, value < centre) {
                (true, _) => 1,
                (false, true) => 0,
                (false, false) => 2,
            }
        };
        self.keys.clear();
        let mut counts = [0; 3];
        for (offset, idx) in window.clone().enumerate() {
            counts[side(idx, self.kept[offset])] += 1;
        }
        for wanted in 0..3 {
            for (offset, idx) in window.clone().enumerate() {
                if side(idx, self.kept[offset]) == wanted {
                    self.keys.push(pack(self.values[idx], self.poss[idx]));
                }
            }
        }
        debug_assert_eq!(self.keys.len(), window.len());
        for (offset, &key) in self.keys.iter().enumerate() {
            self.values[window.start + offset] = f32::from_bits((key >> 32) as u32);
            self.poss[window.start + offset] = key as u32;
        }
        let [low, kept, _] = counts;
        window.start + low..window.start + low + kept
    }

    /// The `count` samples of `window` nearest `centre`: a run of the sorted samples, found by
    /// dropping the farther end until `count` remain. A tie drops the higher end, so the lower
    /// position is kept.
    pub(crate) fn nearest(&self, window: Range<usize>, centre: f32, count: usize) -> Range<usize> {
        let Range { mut start, mut end } = window;
        while end - start > count {
            if (self.values[end - 1] - centre).abs() >= (self.values[start] - centre).abs() {
                end -= 1;
            } else {
                start += 1;
            }
        }
        start..end
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    /// Every permutation of eight distinct values comes out ascending, each with the position it
    /// came from: 8! = 40320 cases. Equal values keep their gather order.
    #[test]
    fn every_permutation_of_eight_sorts_with_its_positions() {
        let mut samples = SortedSamples::default();
        let mut permutation: Vec<f32> = [-3.5, -1.0, -0.0, 0.25, 1.0, 2.0, 7.5, 100.0].to_vec();
        let ascending = permutation.clone();
        let mut count = 0;
        permute(&mut permutation, 0, &mut |values| {
            samples.fill(values);
            assert_eq!(samples.values(), ascending);
            for (value, &pos) in samples.values().iter().zip(samples.poss()) {
                assert_eq!(values[pos as usize].to_bits(), value.to_bits());
            }
            count += 1;
        });
        assert_eq!(count, 40_320);

        samples.fill(&[2.0, 1.0, 2.0, 1.0]);
        assert_eq!(samples.values(), [1.0, 1.0, 2.0, 2.0]);
        assert_eq!(samples.poss(), [1, 3, 0, 2]);
    }

    /// A partition moves each rejected sample to the end on its side of the centre and keeps the
    /// rest ascending between them: of 1 … 6 about 3.5, rejecting 2 and 5, the 2 goes first and
    /// the 5 last, and 1, 3, 4, 6 are the kept run, each still with its position. The band is
    /// asked of each sample once.
    #[test]
    fn a_partition_keeps_the_survivors_one_ascending_run() {
        let mut samples = SortedSamples::default();
        samples.fill(&[6.0, 1.0, 4.0, 2.0, 5.0, 3.0]);
        let calls = Cell::new(0);
        let keep = |value: f32, _: u32| {
            calls.set(calls.get() + 1);
            value != 2.0 && value != 5.0
        };
        assert_eq!(samples.classify(0..6, keep), 4);
        assert_eq!(samples.partition(0..6, 3.5), 1..5);
        assert_eq!(calls.get(), 6, "each sample classified once");
        assert_eq!(samples.values(), [2.0, 1.0, 3.0, 4.0, 6.0, 5.0]);
        assert_eq!(samples.poss(), [3, 1, 5, 2, 0, 4]);
    }

    /// Call `visit` once for every ordering of `values[start..]`, each remaining value swapped to
    /// the front in turn.
    fn permute(values: &mut [f32], start: usize, visit: &mut impl FnMut(&[f32])) {
        if start == values.len() {
            visit(values);
            return;
        }
        for idx in start..values.len() {
            values.swap(start, idx);
            permute(values, start + 1, visit);
            values.swap(start, idx);
        }
    }
}
