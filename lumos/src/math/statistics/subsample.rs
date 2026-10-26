//! [`Subsample`]: the uniform-stride sample the image statistics are estimated from.

use common::par;
use rayon::prelude::*;

/// How many samples an image statistic — a median, a MAD, a sigma-clipped background — is taken
/// from at most. The median of n samples scatters by `1.2533·σ/√n`: 0.13% of σ at a million, far
/// under anything these estimates feed.
pub(crate) const MAX_STATISTIC_SAMPLES: usize = 1_000_000;

/// Every `stride`-th of `len` indices, from 0: `⌈len / cap⌉` apart, so never more than `cap` of
/// them — a stride rounded down would take a whole plane of up to `2·cap − 1` samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Subsample {
    stride: usize,
    count: usize,
}

impl Subsample {
    /// At most `cap` of `len` indices; every one when `len ≤ cap`.
    pub(crate) const fn new(len: usize, cap: usize) -> Self {
        let stride = if len > cap { len.div_ceil(cap) } else { 1 };
        Self {
            stride,
            count: len.div_ceil(stride),
        }
    }

    /// How many indices the sample takes.
    pub(crate) const fn count(self) -> usize {
        self.count
    }

    /// The step between sampled indices, for a caller striding a sequence other than a plane.
    pub(crate) const fn stride(self) -> usize {
        self.stride
    }

    /// The `k`-th sampled index.
    pub(crate) const fn idx(self, k: usize) -> usize {
        k * self.stride
    }

    /// The values of `len` pixels a statistic is taken from, `value` reading each: at most `cap`,
    /// every `stride`-th of the `len − fill_count` pixels `is_fill` does not name, as a fill holds
    /// no data. Fills arrive in regions — a mosaic edge, a coverage gap — so striding the plane
    /// itself would draw its whole quota from one side of a frame that is masked down the other.
    pub(crate) fn data_values(
        len: usize,
        fill_count: usize,
        cap: usize,
        is_fill: impl Fn(usize) -> bool,
        value: impl Fn(usize) -> f32 + Sync,
    ) -> Vec<f32> {
        debug_assert!(fill_count <= len);
        let sample = Self::new(len - fill_count, cap);
        if fill_count == 0 {
            // A stride-`n` walk over several planes costs a cache line and a TLB stream per plane
            // and sampled pixel; running it in parallel hides that latency.
            return par::into_iter(0..sample.count())
                .map(|k| value(sample.idx(k)))
                .collect();
        }
        (0..len)
            .filter(|&idx| !is_fill(idx))
            .step_by(sample.stride())
            .map(value)
            .collect()
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::math::statistics::subsample::Subsample;

    impl Subsample {
        /// The sampled indices, in order.
        pub(crate) fn idxs(self) -> impl Iterator<Item = usize> {
            (0..self.count).map(move |k| self.idx(k))
        }

        /// The sampled values of `plane`, which the sample was made for.
        pub(crate) fn of(self, plane: &[f32]) -> impl Iterator<Item = f32> + '_ {
            debug_assert!(self.count == 0 || self.idx(self.count - 1) < plane.len());
            self.idxs().map(move |idx| plane[idx])
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::math::statistics::subsample::Subsample;

    /// The cap holds at every length around it, and below it every index is taken: 1 000 of a cap
    /// of 1 000 is all of them, 1 001 strides by 2 (501), 1 999 by 2 (1 000), 2 001 by 3 (667) —
    /// where a stride rounded down would take all 1 999.
    #[test]
    fn the_cap_holds_and_the_stride_rounds_up() {
        for (len, stride, count) in [
            (0, 1, 0),
            (5, 1, 5),
            (1000, 1, 1000),
            (1001, 2, 501),
            (1999, 2, 1000),
            (2000, 2, 1000),
            (2001, 3, 667),
        ] {
            let sample = Subsample::new(len, 1000);
            assert_eq!((sample.stride(), sample.count()), (stride, count), "{len}");
            let idxs: Vec<usize> = sample.idxs().collect();
            assert_eq!(idxs.len(), count);
            assert!(idxs.iter().all(|&i| i < len && i % stride == 0));
        }
        let plane: Vec<f32> = (0..7).map(|i| i as f32).collect();
        assert_eq!(
            Subsample::new(7, 3).of(&plane).collect::<Vec<_>>(),
            [0.0, 3.0, 6.0]
        );
    }
}
