//! What one reduced pixel carries out of the combine, and the buffers that get it there.

use crate::combine::rejection::scratch_buffers::ScratchBuffers;
use crate::io::image::pixel_flags::Flags;
use crate::run_report::LocalFlagCounts;

/// The flags a combine leaves a sample out for, while enough unflagged samples remain: each says
/// the value is not the photosite's own measurement.
const SOFT_EXCLUDED: Flags = Flags::SATURATED
    .union(Flags::DEFECT)
    .union(Flags::COSMIC_RAY)
    .union(Flags::REPAIRED)
    .union(Flags::FLAT_FLOOR);

/// Everything one combine job needs beyond the pixels themselves: the covering frames' samples
/// packed to the front, with their weights, flags, frames and noise in the same order, and the
/// rejection methods' own working buffers.
///
/// Leased from a [`JobScratchPool`](crate::concurrency::JobScratchPool) rather than built in a
/// `for_each_init` init closure, because the row loop runs once per chunk per channel — a fresh
/// init would rebuild every vector on each of those.
#[derive(Debug, Default)]
pub(crate) struct CombineScratch {
    pub(super) values: Vec<f32>,
    pub(super) eff_weights: Vec<f32>,
    pub(super) sample_flags: Vec<u8>,
    pub(super) frame_ids: Vec<u32>,
    /// Each sample's noise variance, from [`SampleNoise`](crate::combine::cache::sample_noise::SampleNoise)
    /// over the warp's confidence. Filled only when the combine asked for it.
    pub(super) noise_variances: Vec<f32>,
    pub(super) buffers: ScratchBuffers,
}

/// One pixel's gathered samples, as a reducer reads them.
#[derive(Debug)]
pub(crate) struct PixelSamples<'a> {
    pub(crate) values: &'a mut [f32],
    pub(crate) weights: &'a [f32],
    /// The frame each sample came from.
    pub(crate) frame_ids: &'a [u32],
    /// Each sample's noise variance, when the combine measures a spread.
    pub(crate) noise_variances: Option<&'a [f32]>,
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
        self.noise_variances.resize(frame_count, 0.0);
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
    pub(super) noise_variances: &'a mut [f32],
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
            .filter(|&&byte| Flags::from_byte(byte).intersects(SOFT_EXCLUDED))
            .count();
        if flagged == 0 {
            return count;
        }
        if count - flagged < min_survivors {
            for &byte in &self.sample_flags[..count] {
                kept_flagged.count(Flags::from_byte(byte));
            }
            return count;
        }
        let mut write = 0;
        for read in 0..count {
            let sample_flags = Flags::from_byte(self.sample_flags[read]);
            if sample_flags.intersects(SOFT_EXCLUDED) {
                excluded.count(sample_flags);
            } else {
                self.values[write] = self.values[read];
                self.eff_weights[write] = self.eff_weights[read];
                self.sample_flags[write] = self.sample_flags[read];
                self.frame_ids[write] = self.frame_ids[read];
                self.noise_variances[write] = self.noise_variances[read];
                write += 1;
            }
        }
        write
    }
}

/// One reduced channel sample: the combined value, how many samples reached it, and — when the
/// caller asked for the quality planes — the effective weight of the survivors.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct CombinedSample {
    pub(crate) value: f32,
    /// Samples that survived rejection. Always tracked: it is a count the reducer already knows,
    /// and quantization-noise propagation keys on it.
    pub(crate) survivor_count: usize,
    pub(crate) weight: f32,
    pub(crate) linear_variance: f32,
}

impl CombinedSample {
    /// A reduction whose survivors are all the inputs.
    pub(crate) fn from_all(value: f32, weights: &[f32]) -> Self {
        Self::from_survivors(value, weights, weights.len(), 0..weights.len())
    }

    /// A reduction over `survivor_indices` into `weights`, measuring their effective weight.
    pub(crate) fn from_survivors(
        value: f32,
        weights: &[f32],
        survivor_count: usize,
        survivor_indices: impl IntoIterator<Item = usize>,
    ) -> Self {
        let mut weight = 0.0f32;
        let mut weight_squared = 0.0f32;
        for index in survivor_indices {
            let survivor_weight = weights[index];
            weight += survivor_weight;
            weight_squared += survivor_weight * survivor_weight;
        }
        let linear_variance = if weight > 0.0 {
            weight_squared / (weight * weight)
        } else {
            0.0
        };
        Self {
            value,
            survivor_count,
            weight,
            linear_variance,
        }
    }

    /// A reduction for a combine that asked for no quality planes: the walk over survivor weights
    /// would produce two numbers nothing reads, and it costs one pass over the frames per pixel.
    pub(crate) const fn value_only(value: f32, survivor_count: usize) -> Self {
        Self {
            value,
            survivor_count,
            weight: 0.0,
            linear_variance: 0.0,
        }
    }
}
