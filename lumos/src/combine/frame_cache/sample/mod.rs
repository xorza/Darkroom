//! What one reduced pixel carries out of the combine, and the buffers that get it there.

use crate::combine::rejection::scratch_buffers::ScratchBuffers;
use crate::image::flag_counts::FlagCounts;
use crate::image::quality_flags::QualityFlags;
use crate::scratch_pool::Scratch;

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
/// Leased from a [`ScratchPool`](crate::scratch_pool::ScratchPool) shaped by the frame count,
/// rather than built in a `for_each_init` init closure, because the row loop runs once per chunk
/// per channel — a fresh init would rebuild every vector on each of those. The gather buffers are
/// indexed directly, so they hold the frame count as their length, not just as capacity.
#[derive(Debug, Default)]
pub(crate) struct CombineScratch {
    pub(super) values: Vec<f32>,
    pub(super) eff_weights: Vec<f32>,
    pub(super) sample_flags: Vec<u8>,
    /// The columns of [`NoiseCols`](crate::combine::reducer::pixel_samples::NoiseCols),
    /// filled only when the combine asked for noise.
    pub(super) noise_bg: Vec<f32>,
    pub(super) noise_sky: Vec<f32>,
    pub(super) noise_per_unit: Vec<f32>,
    pub(super) buffers: ScratchBuffers,
}

impl CombineScratch {
    /// The bytes one scratch holds at most for `frame_count` frames: a value, weight and flag
    /// byte and three noise columns a frame, and the rejection methods' buffers.
    pub(crate) const fn heap_bytes(frame_count: usize) -> usize {
        frame_count
            .saturating_mul(5 * size_of::<f32>() + size_of::<u8>())
            .saturating_add(ScratchBuffers::heap_bytes(frame_count))
    }
}

impl Scratch for CombineScratch {
    /// The frame count.
    type Shape = usize;

    fn build(frame_count: usize) -> Self {
        let mut scratch = Self::default();
        scratch.fit(frame_count);
        scratch
    }

    fn fit(&mut self, frame_count: usize) {
        fit_column(&mut self.values, frame_count);
        fit_column(&mut self.eff_weights, frame_count);
        fit_column(&mut self.sample_flags, frame_count);
        fit_column(&mut self.noise_bg, frame_count);
        fit_column(&mut self.noise_sky, frame_count);
        fit_column(&mut self.noise_per_unit, frame_count);
        self.buffers.reserve(frame_count);
    }

    fn has_shape(&self, frame_count: usize) -> bool {
        [
            self.values.len(),
            self.eff_weights.len(),
            self.sample_flags.len(),
            self.noise_bg.len(),
            self.noise_sky.len(),
            self.noise_per_unit.len(),
        ] == [frame_count; 6]
    }
}

/// `column` at `len` samples, its allocation exactly that when it grows, as
/// [`CombineScratch::heap_bytes`] counts it: `resize` would round a small count up.
fn fit_column<T: Clone + Default>(column: &mut Vec<T>, len: usize) {
    column.reserve_exact(len.saturating_sub(column.len()));
    column.resize(len, T::default());
}

/// The flagged samples a combine left out, and those it had to keep.
#[derive(Debug, Default)]
pub(super) struct FlagTallies {
    pub(super) excluded: FlagCounts,
    pub(super) kept: FlagCounts,
}

/// One pixel's gather buffers, borrowed as slices: the gather loop stores into these, so it holds
/// them as plain slices rather than reaching through the scratch for every sample.
#[derive(Debug)]
pub(super) struct GatheredSamples<'a> {
    pub(super) values: &'a mut [f32],
    pub(super) eff_weights: &'a mut [f32],
    pub(super) sample_flags: &'a mut [u8],
    pub(super) noise_bg: &'a mut [f32],
    pub(super) noise_sky: &'a mut [f32],
    pub(super) noise_per_unit: &'a mut [f32],
}

impl GatheredSamples<'_> {
    /// Leave out the first `count` samples' flagged ones, those [`SOFT_EXCLUDED`] names, keeping
    /// the rest in order with everything gathered beside them, when at least `min_survivors`
    /// unflagged samples remain; otherwise keep every sample. Returns how many samples stay at the
    /// front, and counts each flagged sample in `tallies` as left out or as kept.
    pub(super) fn leave_out_flagged(
        &mut self,
        count: usize,
        min_survivors: usize,
        tallies: &mut FlagTallies,
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
                tallies
                    .kept
                    .tally(QualityFlags::from_byte(byte).intersection(SOFT_EXCLUDED));
            }
            return count;
        }
        let mut write = 0;
        for read in 0..count {
            let sample_flags = QualityFlags::from_byte(self.sample_flags[read]);
            if sample_flags.intersects(SOFT_EXCLUDED) {
                tallies
                    .excluded
                    .tally(sample_flags.intersection(SOFT_EXCLUDED));
            } else {
                self.values[write] = self.values[read];
                self.eff_weights[write] = self.eff_weights[read];
                self.sample_flags[write] = self.sample_flags[read];
                self.noise_bg[write] = self.noise_bg[read];
                self.noise_sky[write] = self.noise_sky[read];
                self.noise_per_unit[write] = self.noise_per_unit[read];
                write += 1;
            }
        }
        write
    }
}

#[cfg(feature = "internals")]
pub(crate) mod bench;
