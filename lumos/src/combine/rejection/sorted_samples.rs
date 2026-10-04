//! [`SortedSamples`]: one pixel's samples in ascending order, each with the gather position it came
//! from.

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
    positions: Vec<u32>,
}

impl SortedSamples {
    /// Reserve room for `count` samples, so the per-pixel refill never allocates.
    pub(crate) fn reserve(&mut self, count: usize) {
        self.keys.reserve(count);
        self.values.reserve(count);
        self.positions.reserve(count);
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
                .map(|(position, &value)| u64::from(order_key(value)) << 32 | position as u64),
        );
        self.keys.sort_unstable();
        self.values.clear();
        self.positions.clear();
        for &key in &self.keys {
            let position = key as u32;
            self.values.push(values[position as usize]);
            self.positions.push(position);
        }
    }

    pub(crate) fn values(&self) -> &[f32] {
        &self.values
    }

    /// Each sorted sample's position in the gather it came from.
    pub(crate) fn positions(&self) -> &[u32] {
        &self.positions
    }
}

/// An unsigned integer that orders as the float does: a positive float's bits order as its value
/// once the sign bit is set, and a negative float's order backwards, so all of its bits flip.
const fn order_key(value: f32) -> u32 {
    let bits = value.to_bits();
    if bits >> 31 == 1 {
        !bits
    } else {
        bits | 1 << 31
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Keys order as the floats: negative and positive, zeros of both signs, subnormals and the
    /// largest magnitudes.
    #[test]
    fn keys_order_as_the_floats() {
        let ascending = [
            f32::MIN,
            -1.0,
            -f32::MIN_POSITIVE,
            -1e-45,
            -0.0,
            0.0,
            1e-45,
            f32::MIN_POSITIVE,
            1.0,
            f32::MAX,
        ];
        for pair in ascending.windows(2) {
            assert!(order_key(pair[0]) < order_key(pair[1]), "{pair:?}");
        }
    }

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
            for (value, &position) in samples.values().iter().zip(samples.positions()) {
                assert_eq!(values[position as usize].to_bits(), value.to_bits());
            }
            count += 1;
        });
        assert_eq!(count, 40_320);

        samples.fill(&[2.0, 1.0, 2.0, 1.0]);
        assert_eq!(samples.values(), [1.0, 1.0, 2.0, 2.0]);
        assert_eq!(samples.positions(), [1, 3, 0, 2]);
    }

    /// Call `visit` once for every ordering of `values[start..]`, each remaining value swapped to
    /// the front in turn.
    fn permute(values: &mut [f32], start: usize, visit: &mut impl FnMut(&[f32])) {
        if start == values.len() {
            visit(values);
            return;
        }
        for index in start..values.len() {
            values.swap(start, index);
            permute(values, start + 1, visit);
            values.swap(start, index);
        }
    }
}
