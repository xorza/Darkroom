//! [`BinnedSamples`]: a frame's noise samples, grouped by the bin of flat gain each was taken at.

use std::ops::Range;
use std::{array, mem};

use arrayvec::ArrayVec;
use rayon::prelude::*;

use crate::image::flat_gain::gain_bins::{GAIN_BINS, GainBins};

/// The fewest samples a bin is measured over: a bin with fewer merges with its neighbours in gain.
/// A variance over 256 samples carries a standard error of √(2/256) ≈ 9%.
const MIN_BIN_SAMPLES: usize = 256;

/// How many samples one parallel share of [`BinnedSamples::of`] tags.
const BIN_SHARE: usize = 1 << 14;

/// The noise measured over the samples of one bin of flat gain.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct GainBin {
    /// The bin's mean gain.
    pub(crate) gain: f64,
    pub(crate) variance: f64,
    pub(crate) samples: usize,
}

/// Samples [`BinnedSamples`] holds and groups by bin in place: one buffer, or columns that are
/// permuted together.
pub(crate) trait Samples {
    /// A run of consecutive samples, as a measure reads it.
    type Run<'a>: Send
    where
        Self: 'a;

    /// The bytes one sample holds.
    const BYTES: usize;

    fn len(&self) -> usize;

    fn swap(&mut self, a: usize, b: usize);

    /// The runs `ranges` cover, which ascend and do not overlap.
    fn runs(&mut self, ranges: &[Range<usize>]) -> Vec<Self::Run<'_>>;
}

impl<T: Send> Samples for Vec<T> {
    type Run<'a>
        = &'a mut [T]
    where
        T: 'a;

    const BYTES: usize = size_of::<T>();

    fn len(&self) -> usize {
        <[T]>::len(self)
    }

    fn swap(&mut self, a: usize, b: usize) {
        <[T]>::swap(self, a, b);
    }

    fn runs(&mut self, ranges: &[Range<usize>]) -> Vec<&mut [T]> {
        let mut runs = Vec::with_capacity(ranges.len());
        let mut rest = self.as_mut_slice();
        let mut offset = 0;
        for range in ranges {
            let (_, tail) = mem::take(&mut rest).split_at_mut(range.start - offset);
            let (run, tail) = tail.split_at_mut(range.len());
            runs.push(run);
            rest = tail;
            offset = range.end;
        }
        runs
    }
}

/// A frame's noise samples, each tagged with its gain bin, and each bin's sum of gains: what an
/// estimator fills before it measures each bin's variance. The samples stay in the buffers the
/// caller sized, and [`Self::measure`] groups them by bin in place, so the samples are held once.
#[derive(Debug)]
pub(crate) struct BinnedSamples<S> {
    samples: S,
    bins: Vec<u8>,
    gain_sums: [f64; GAIN_BINS],
}

impl<T> BinnedSamples<Vec<T>> {
    /// Room for `capacity` samples, more than which [`Self::push`] never takes.
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            samples: Vec::with_capacity(capacity),
            bins: Vec::with_capacity(capacity),
            gain_sums: [0.0; GAIN_BINS],
        }
    }

    pub(crate) fn push(&mut self, bins: &GainBins, gain: f32, sample: T) {
        debug_assert!(
            self.samples.len() < self.samples.capacity(),
            "a sample past the room the caller sized"
        );
        let bin = bins.bin(gain);
        self.samples.push(sample);
        self.bins.push(bin as u8);
        self.gain_sums[bin] += f64::from(gain);
    }
}

impl<S: Samples> BinnedSamples<S> {
    /// `samples`, each in the bin of the gain `gain` reads for the sample at its position: tagged
    /// in parallel shares of [`BIN_SHARE`], each summing its own gains, the shares' sums added in
    /// order so the bits do not depend on the thread count.
    pub(crate) fn of(samples: S, bins: &GainBins, gain: impl Fn(&S, usize) -> f32 + Sync) -> Self
    where
        S: Sync,
    {
        let mut tags = vec![0u8; samples.len()];
        let mut shares = Vec::with_capacity(tags.len().div_ceil(BIN_SHARE));
        tags.par_chunks_mut(BIN_SHARE)
            .enumerate()
            .map(|(share, tags)| {
                let start = share * BIN_SHARE;
                let mut sums = [0.0f64; GAIN_BINS];
                for (offset, tag) in tags.iter_mut().enumerate() {
                    let gain = gain(&samples, start + offset);
                    let bin = bins.bin(gain);
                    sums[bin] += f64::from(gain);
                    *tag = bin as u8;
                }
                sums
            })
            .collect_into_vec(&mut shares);
        let mut gain_sums = [0.0; GAIN_BINS];
        for share in shares {
            for (sum, part) in gain_sums.iter_mut().zip(share) {
                *sum += part;
            }
        }
        Self {
            samples,
            bins: tags,
            gain_sums,
        }
    }

    /// The bytes `count` samples hold here: each sample and its bin.
    pub(crate) const fn heap_bytes(count: usize) -> usize {
        count * (S::BYTES + size_of::<u8>())
    }

    /// The bins `variance` measures: each run of bins in gain order merged until it holds
    /// [`MIN_BIN_SAMPLES`], a short last run into the one before it; one run of every sample when
    /// all of them are fewer. A frame too small to fill the bins still has its noise measured,
    /// over coarser runs of gain. Within a run the samples come in no particular order. The runs
    /// are measured in parallel, each on its own.
    pub(crate) fn measure(
        mut self,
        variance: impl Fn(S::Run<'_>) -> f64 + Sync,
    ) -> ArrayVec<GainBin, GAIN_BINS> {
        let mut counts = [0usize; GAIN_BINS];
        for &bin in &self.bins {
            counts[usize::from(bin)] += 1;
        }
        self.group(&counts);
        let mut runs: ArrayVec<(Range<usize>, f64), GAIN_BINS> = ArrayVec::new();
        let (mut start, mut end, mut sum) = (0, 0, 0.0);
        for (count, gain_sum) in counts.into_iter().zip(self.gain_sums) {
            end += count;
            sum += gain_sum;
            if end - start >= MIN_BIN_SAMPLES {
                runs.push((start..end, sum));
                (start, sum) = (end, 0.0);
            }
        }
        if end > start {
            match runs.last_mut() {
                Some((last, last_sum)) => {
                    last.end = end;
                    *last_sum += sum;
                }
                None => runs.push((start..end, sum)),
            }
        }
        let ranges: ArrayVec<Range<usize>, GAIN_BINS> =
            runs.iter().map(|(range, _)| range.clone()).collect();
        let mut variances = Vec::with_capacity(ranges.len());
        self.samples
            .runs(&ranges)
            .into_par_iter()
            .map(&variance)
            .collect_into_vec(&mut variances);
        runs.into_iter()
            .zip(variances)
            .map(|((range, gain_sum), variance)| GainBin {
                gain: gain_sum / range.len() as f64,
                samples: range.len(),
                variance,
            })
            .collect()
    }

    /// Put the samples in bin order, in place: each bin's `counts` share, in turn, swaps every
    /// sample it holds that belongs elsewhere to the next open place of that sample's bin.
    fn group(&mut self, counts: &[usize; GAIN_BINS]) {
        let mut next = [0usize; GAIN_BINS];
        for bin in 1..GAIN_BINS {
            next[bin] = next[bin - 1] + counts[bin - 1];
        }
        let ends: [usize; GAIN_BINS] = array::from_fn(|bin| next[bin] + counts[bin]);
        for bin in 0..GAIN_BINS {
            while next[bin] < ends[bin] {
                let at = next[bin];
                let home = usize::from(self.bins[at]);
                if home == bin {
                    next[bin] += 1;
                } else {
                    let to = next[home];
                    self.samples.swap(at, to);
                    self.bins.swap(at, to);
                    next[home] += 1;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::image::flat_gain::gain_bins::GainBins;
    use crate::noise::binned_samples::{BinnedSamples, GainBin};

    /// Bins under 256 samples merge with their neighbours up in gain: over bounds 3, 5, …, 15, 300
    /// samples at gain 1 fill bin 0 alone; 100 each at gains 3, 5 and 7 fill bins 1 to 3 only together,
    /// 300; and the 50 at gain 15 in bin 7, short at the end, join that run, 350 samples of mean gain
    /// (300 + 500 + 700 + 750)/350 = 2250/350. Each sample is its own gain and they are pushed
    /// interleaved, so a run that sums to its gains holds its own samples and no other: 300 × 1 and
    /// 100 × (3 + 5 + 7) + 50 × 15 = 2250. A hundred samples in all are one run.
    #[test]
    fn short_bins_merge_up_in_gain() {
        let mut gains: Vec<f32> = (1..=16).map(|gain| gain as f32).collect();
        let bins = GainBins::of(&mut gains);
        let mut binned = BinnedSamples::with_capacity(650);
        let mut left = [
            (1.0f32, 300),
            (3.0, 100),
            (5.0, 100),
            (7.0, 100),
            (15.0, 50),
        ];
        while left.iter().any(|&(_, count)| count > 0) {
            for (gain, count) in left.iter_mut().rev().filter(|(_, count)| *count > 0) {
                binned.push(&bins, *gain, *gain);
                *count -= 1;
            }
        }
        let measured = binned.measure(|samples| samples.iter().map(|&gain| f64::from(gain)).sum());
        assert_eq!(
            measured.as_slice(),
            [
                GainBin {
                    gain: 1.0,
                    variance: 300.0,
                    samples: 300,
                },
                GainBin {
                    gain: 2250.0 / 350.0,
                    variance: 2250.0,
                    samples: 350,
                },
            ]
        );

        let mut few = BinnedSamples::with_capacity(100);
        for _ in 0..100 {
            few.push(&bins, 1.0, 1.0f32);
        }
        let measured = few.measure(|samples| samples.len() as f64);
        assert_eq!(measured.len(), 1);
        assert_eq!(measured[0].samples, 100);
    }
}
