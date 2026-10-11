//! [`BackgroundSplit`]: a frame's background noise split by how a flat amplified it.

use std::array;
use std::ops::Range;

use arrayvec::ArrayVec;
use serde::{Deserialize, Serialize};

/// The background variance of a flat-divided frame: `σ²·(ρ·g² + (1 − ρ)·g)` at a pixel the flat
/// multiplied by `g = 1/f`.
///
/// The read noise and the dark current were added before the flat shaped the light, so the
/// division scales their variance `A` by `g²`. The sky's photons arrived through the vignetting:
/// their count, and with it their variance, is `f` times the unvignetted one, which the division
/// leaves at `S·g`. That is the CCD equation carried through a flat, as ccdproc propagates it. Most
/// frames state no gain to separate the two by, so the frame's own noise, measured over bins of
/// `g`, is fitted by `A·g² + S·g` with both terms at least 0; then `σ² = A + S` and
/// `ρ = A / (A + S)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct BackgroundSplit {
    /// The variance where the flat did nothing, `g = 1`.
    pub(crate) variance: f32,
    /// The share of [`Self::variance`] the flat amplifies twice.
    pub(crate) read_share: f32,
}

/// The noise measured over the samples of one bin of flat gain.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GainBin {
    /// The bin's mean gain.
    pub(crate) gain: f64,
    pub(crate) variance: f64,
    pub(crate) samples: usize,
}

/// Bins of equal count a frame's noise is measured over, by flat gain: enough to fit two terms
/// with residuals to spare, each holding an eighth of the samples.
pub(crate) const GAIN_BINS: usize = 8;

const _: () = assert!(GAIN_BINS <= 1 << u8::BITS, "a sample's bin is a byte");

/// The fewest samples a bin is measured over: a bin with fewer merges with its neighbours in gain.
/// A variance over 256 samples carries a standard error of √(2/256) ≈ 9%.
pub(crate) const MIN_BIN_SAMPLES: usize = 256;

/// A frame's noise samples, each tagged with its gain bin, and each bin's sum of gains: what an
/// estimator fills before it measures each bin's variance. The samples stay in the one buffer the
/// caller sized, and [`Self::measure`] groups them by bin in place, so the samples are held once.
#[derive(Debug)]
pub(crate) struct BinnedSamples<T> {
    samples: Vec<T>,
    bins: Vec<u8>,
    gain_sums: [f64; GAIN_BINS],
}

impl<T> BinnedSamples<T> {
    /// Room for `capacity` samples, more than which [`Self::push`] never takes.
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            samples: Vec::with_capacity(capacity),
            bins: Vec::with_capacity(capacity),
            gain_sums: [0.0; GAIN_BINS],
        }
    }

    /// `samples`, each in the bin of the gain `gain` reads for it.
    pub(crate) fn of(samples: Vec<T>, bins: &GainBins, gain: impl Fn(&T) -> f32) -> Self {
        let mut gain_sums = [0.0; GAIN_BINS];
        let bins = samples
            .iter()
            .map(|sample| {
                let gain = gain(sample);
                let bin = bins.bin(gain);
                gain_sums[bin] += f64::from(gain);
                bin as u8
            })
            .collect();
        Self {
            samples,
            bins,
            gain_sums,
        }
    }

    /// The bytes `count` samples hold here: each sample and its bin.
    pub(crate) const fn heap_bytes(count: usize) -> usize {
        count * (size_of::<T>() + size_of::<u8>())
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

    /// The bins `variance` measures: each run of bins in gain order merged until it holds
    /// [`MIN_BIN_SAMPLES`], a short last run into the one before it; one run of every sample when
    /// all of them are fewer. A frame too small to fill the bins still has its noise measured,
    /// over coarser runs of gain. Within a run the samples come in no particular order.
    pub(crate) fn measure(mut self, mut variance: impl FnMut(&mut [T]) -> f64) -> Vec<GainBin> {
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
        runs.into_iter()
            .map(|(run, gain_sum)| GainBin {
                gain: gain_sum / run.len() as f64,
                samples: run.len(),
                variance: variance(&mut self.samples[run]),
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

impl BackgroundSplit {
    /// A frame no flat divided: its measured variance, which no gain scales.
    pub(crate) const fn unflattened(variance: f32) -> Self {
        Self {
            variance,
            read_share: 0.0,
        }
    }

    /// The variance at flat gain `gain`.
    pub(crate) fn at(self, gain: f32) -> f32 {
        self.variance * gain * (self.read_share * gain + 1.0 - self.read_share)
    }

    /// The weighted least-squares fit of `A·g² + S·g` to `bins`, both terms at least 0, each bin
    /// weighed by its samples over its variance squared: the inverse of its estimate's sampling
    /// variance, `2v²/n` for Gaussian noise.
    ///
    /// When the unconstrained solution leaves a term negative, the better of the two one-term fits
    /// stands. Where the gain hardly varies, the two terms cannot be told apart — and there the
    /// split changes no variance the frame holds — so a tie goes to the sky-limited `S·g`, the
    /// exposure most deep-sky frames are taken at. No bin at all reads 0.
    pub(crate) fn fit(bins: &[GainBin]) -> Self {
        let bins: Vec<(f64, f64, f64)> = bins
            .iter()
            .filter(|bin| bin.samples > 0)
            .map(|bin| {
                let weight = if bin.variance > 0.0 {
                    bin.samples as f64 / (bin.variance * bin.variance)
                } else {
                    bin.samples as f64
                };
                (bin.gain, bin.variance, weight)
            })
            .collect();
        if bins.is_empty() {
            return Self::unflattened(0.0);
        }
        let sum = |term: &dyn Fn(f64, f64) -> f64| {
            bins.iter()
                .map(|&(gain, variance, weight)| weight * term(gain, variance))
                .sum::<f64>()
        };
        let g4 = sum(&|g, _| g.powi(4));
        let g3 = sum(&|g, _| g.powi(3));
        let g2 = sum(&|g, _| g * g);
        let vg2 = sum(&|g, v| v * g * g);
        let vg = sum(&|g, v| v * g);
        let residual = |read: f64, sky: f64| sum(&|g, v| (v - read * g * g - sky * g).powi(2));
        let sky_only = (0.0, vg / g2);
        let read_only = (vg2 / g4, 0.0);
        let one_term = if residual(read_only.0, read_only.1) < residual(sky_only.0, sky_only.1) {
            read_only
        } else {
            sky_only
        };
        let determinant = g4 * g2 - g3 * g3;
        // Each sum rounds by parts in 10¹⁶; a determinant within 10⁻¹² of the products it is the
        // difference of is a gain spread under 10⁻⁶, where the two-term solve reads rounding.
        let (read, sky) = if determinant > 1e-12 * g4 * g2 {
            let read = (vg2 * g2 - vg * g3) / determinant;
            let sky = (g4 * vg - g3 * vg2) / determinant;
            if read >= 0.0 && sky >= 0.0 {
                (read, sky)
            } else {
                one_term
            }
        } else {
            one_term
        };
        let variance = read + sky;
        Self {
            variance: variance as f32,
            read_share: if variance > 0.0 {
                (read / variance) as f32
            } else {
                0.0
            },
        }
    }
}

#[cfg(test)]
mod tests;
