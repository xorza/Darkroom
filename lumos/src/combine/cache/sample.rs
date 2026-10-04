//! What one reduced pixel carries out of the combine, and the buffers that get it there.

use crate::combine::cache::sample_noise::NoiseColumns;
use crate::combine::rejection::scratch_buffers::ScratchBuffers;
use crate::io::image::pixel_flags::QualityFlags;
use crate::run_report::LocalFlagCounts;

/// The flags a combine leaves a sample out for, while enough unflagged samples remain: each says
/// the value is not the photosite's own measurement.
const SOFT_EXCLUDED: QualityFlags = QualityFlags::SATURATED
    .union(QualityFlags::DEFECT)
    .union(QualityFlags::COSMIC_RAY)
    .union(QualityFlags::REPAIRED)
    .union(QualityFlags::FLAT_FLOOR);

/// Everything one combine job needs beyond the pixels themselves: the covering frames' samples
/// packed to the front, with their weights, flags, frames and noise in the same order, and the
/// rejection methods' own working buffers.
///
/// Leased from a [`JobScratchPool`](crate::concurrency::job_scratch_pool::JobScratchPool) rather than built in a
/// `for_each_init` init closure, because the row loop runs once per chunk per channel — a fresh
/// init would rebuild every vector on each of those.
#[derive(Debug, Default)]
pub(crate) struct CombineScratch {
    pub(super) values: Vec<f32>,
    pub(super) eff_weights: Vec<f32>,
    pub(super) sample_flags: Vec<u8>,
    pub(super) frame_ids: Vec<u32>,
    /// The columns of [`NoiseColumns`], filled only when the combine asked for noise.
    pub(super) noise_background: Vec<f32>,
    pub(super) noise_sky: Vec<f32>,
    pub(super) noise_inverse_electrons: Vec<f32>,
    pub(super) buffers: ScratchBuffers,
}

/// One pixel's gathered samples, as a reducer reads them.
#[derive(Debug)]
pub(crate) struct PixelSamples<'a> {
    pub(crate) values: &'a mut [f32],
    pub(crate) weights: &'a [f32],
    /// The frame each sample came from.
    pub(crate) frame_ids: &'a [u32],
    /// Each sample's noise model, when the combine measures a spread or a variance.
    pub(crate) noise: Option<NoiseColumns<'a>>,
    pub(crate) channel: usize,
}

impl CombineScratch {
    /// Size the gather buffers for `frame_count` frames. They are indexed directly, so they need
    /// the length, not just the capacity.
    pub(super) fn resize(&mut self, frame_count: usize) {
        self.values.resize(frame_count, 0.0);
        self.eff_weights.resize(frame_count, 0.0);
        self.sample_flags.resize(frame_count, 0);
        self.frame_ids.resize(frame_count, 0);
        self.noise_background.resize(frame_count, 0.0);
        self.noise_sky.resize(frame_count, 0.0);
        self.noise_inverse_electrons.resize(frame_count, 0.0);
        self.buffers.reserve(frame_count);
    }
}

/// One pixel's gather buffers, borrowed as slices: the gather loop stores into these, so it holds
/// them as plain slices rather than reaching through the scratch for every sample.
#[derive(Debug)]
pub(super) struct GatheredSamples<'a> {
    pub(super) values: &'a mut [f32],
    pub(super) eff_weights: &'a mut [f32],
    pub(super) sample_flags: &'a mut [u8],
    pub(super) frame_ids: &'a mut [u32],
    pub(super) noise_background: &'a mut [f32],
    pub(super) noise_sky: &'a mut [f32],
    pub(super) noise_inverse_electrons: &'a mut [f32],
}

impl GatheredSamples<'_> {
    /// Leave out the first `count` samples' flagged ones, those [`SOFT_EXCLUDED`] names, keeping
    /// the rest in order with everything gathered beside them, when at least `min_survivors`
    /// unflagged samples remain; otherwise keep every sample. Returns how many samples stay at the
    /// front, and counts each flagged sample as left out or as kept.
    pub(super) fn leave_out_flagged(
        &mut self,
        count: usize,
        min_survivors: usize,
        excluded: &mut LocalFlagCounts,
        kept_flagged: &mut LocalFlagCounts,
    ) -> usize {
        let flagged = self.sample_flags[..count]
            .iter()
            .filter(|&&byte| QualityFlags::from_byte(byte).intersects(SOFT_EXCLUDED))
            .count();
        if flagged == 0 {
            return count;
        }
        if count - flagged < min_survivors {
            for &byte in &self.sample_flags[..count] {
                kept_flagged.count(QualityFlags::from_byte(byte));
            }
            return count;
        }
        let mut write = 0;
        for read in 0..count {
            let sample_flags = QualityFlags::from_byte(self.sample_flags[read]);
            if sample_flags.intersects(SOFT_EXCLUDED) {
                excluded.count(sample_flags);
            } else {
                self.values[write] = self.values[read];
                self.eff_weights[write] = self.eff_weights[read];
                self.sample_flags[write] = self.sample_flags[read];
                self.frame_ids[write] = self.frame_ids[read];
                self.noise_background[write] = self.noise_background[read];
                self.noise_sky[write] = self.noise_sky[read];
                self.noise_inverse_electrons[write] = self.noise_inverse_electrons[read];
                write += 1;
            }
        }
        write
    }
}

/// One reduced channel sample: the combined value, how many samples reached it, and — when the
/// caller asked for the quality planes — the survivors' weight, the variance of the value and the
/// survivors' dispersion.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CombinedSample {
    pub(crate) value: f32,
    /// Samples that survived rejection. Always tracked: it is a count the reducer already knows,
    /// and quantization-noise propagation keys on it.
    pub(crate) survivor_count: usize,
    pub(crate) weight: f32,
    pub(crate) variance: f32,
    pub(crate) dispersion: f32,
}

impl CombinedSample {
    /// A weighted mean `value` over the samples at `survivors`: the weight is `Σwᵢ`, and the
    /// variance is `Σwᵢ²·vᵢ / (Σwᵢ)²` with each sample's model variance `vᵢ` taken at the combined
    /// value, the estimate of the true signal. Taken at each sample's own value instead, an upward
    /// fluctuation would carry a larger variance and pull the figure up. Without noise columns the
    /// variance reads 0, for a reducer whose request has no variance plane.
    ///
    /// The dispersion is the variance of the same mean as the samples' scatter shows it, with no
    /// noise model: `Σwᵢ(xᵢ − x̄)² / ((n − 1)·Σwᵢ)`. Where each sample's variance is `c / wᵢ`, the
    /// weighted sum of squares has expectation `(n − 1)·c` and the mean's variance is `c / Σwᵢ`, so
    /// the figure is unbiased; noise weighting makes the weights so at the sky, and equal weights
    /// make it the squared standard error of the mean. NaN for fewer than two survivors, whose
    /// scatter says nothing.
    pub(crate) fn from_survivors(
        value: f32,
        values: &[f32],
        weights: &[f32],
        survivors: impl IntoIterator<Item = usize>,
        noise: Option<NoiseColumns<'_>>,
    ) -> Self {
        let mut count = 0usize;
        let mut weight = 0.0f32;
        let mut weighted_variance = 0.0f32;
        let mut weighted_squares = 0.0f32;
        for index in survivors {
            let survivor_weight = weights[index];
            count += 1;
            weight += survivor_weight;
            let deviation = values[index] - value;
            weighted_squares += survivor_weight * deviation * deviation;
            if let Some(noise) = noise {
                weighted_variance +=
                    survivor_weight * survivor_weight * noise.variance_at(index, value);
            }
        }
        Self {
            value,
            survivor_count: count,
            weight,
            variance: if weight > 0.0 {
                weighted_variance / (weight * weight)
            } else {
                0.0
            },
            dispersion: if count > 1 && weight > 0.0 {
                weighted_squares / ((count - 1) as f32 * weight)
            } else {
                f32::NAN
            },
        }
    }

    /// A pixel no sample reached: nothing combined, nothing weighed, and no scatter to read.
    pub(crate) const fn uncovered() -> Self {
        Self {
            value: 0.0,
            survivor_count: 0,
            weight: 0.0,
            variance: 0.0,
            dispersion: f32::NAN,
        }
    }

    /// A reduction for a combine that asked for no quality planes: the walk over survivor weights
    /// would produce two numbers nothing reads, and it costs one pass over the frames per pixel.
    pub(crate) const fn value_only(value: f32, survivor_count: usize) -> Self {
        Self {
            value,
            survivor_count,
            weight: 0.0,
            variance: 0.0,
            dispersion: 0.0,
        }
    }
}
