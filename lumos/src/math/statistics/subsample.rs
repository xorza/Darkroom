//! [`Subsample`]: the uniform-stride sample the image statistics are estimated from.

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
    pub(crate) const fn index(self, k: usize) -> usize {
        k * self.stride
    }

    /// The sampled indices, in order.
    pub(crate) fn indices(self) -> impl Iterator<Item = usize> {
        (0..self.count).map(move |k| self.index(k))
    }

    /// The sampled values of `plane`, which the sample was made for.
    pub(crate) fn of(self, plane: &[f32]) -> impl Iterator<Item = f32> + '_ {
        debug_assert!(self.count == 0 || self.index(self.count - 1) < plane.len());
        self.indices().map(move |index| plane[index])
    }

    /// The values of `plane` an image statistic is taken from: at most [`MAX_STATISTIC_SAMPLES`].
    pub(crate) fn statistic_values(plane: &[f32]) -> Vec<f32> {
        Self::new(plane.len(), MAX_STATISTIC_SAMPLES)
            .of(plane)
            .collect()
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
            let indices: Vec<usize> = sample.indices().collect();
            assert_eq!(indices.len(), count);
            assert!(indices.iter().all(|&i| i < len && i % stride == 0));
        }
        let plane: Vec<f32> = (0..7).map(|i| i as f32).collect();
        assert_eq!(
            Subsample::new(7, 3).of(&plane).collect::<Vec<_>>(),
            [0.0, 3.0, 6.0]
        );
    }
}
