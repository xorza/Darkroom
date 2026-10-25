//! [`PlannedNormalization`]: a normalization as a memory plan prices it, before its frames exist.

use arrayvec::ArrayVec;
use imaginarium::PlanarDesc;

use crate::combine::frame_read::slots::Slots;
use crate::combine::normalization::Normalization;
use crate::combine::normalization::common_domain::CommonDomain;
use crate::frame_store::stratified_samples::StratifiedSamples;
use crate::math::statistics::radix_median::RadixMedian;

/// How a set's frames cover the pixels, which decides where
/// [`FrameNorm::measure`](crate::combine::normalization::FrameNorm::measure) measures them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SetCoverage {
    /// Each slot is measured over the pixels every frame covers, or over a stratified sample of
    /// them all when every frame covers every pixel, which holds less.
    Common,
    /// Each frame is gathered in each channel by its own rule: measured pair by pair.
    PerChannel,
}

/// A normalization a memory plan prices before its frames exist: `frame_count` frames of
/// `desc` read in `slots`, covering the pixels as `coverage` says, measured by `threads`
/// workers.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PlannedNormalization {
    pub(crate) frame_count: usize,
    pub(crate) desc: PlanarDesc,
    pub(crate) slots: Slots,
    pub(crate) normalization: Normalization,
    pub(crate) coverage: SetCoverage,
    pub(crate) threads: usize,
}

impl PlannedNormalization {
    /// The most bytes [`FrameNorm::measure`](crate::combine::normalization::FrameNorm::measure)
    /// holds.
    ///
    /// Each slot is measured over at most [`SAMPLE_LIMIT`] of its pixels. Over the pixels the
    /// frames share, the run holds the domain's mask, one more per color of a mosaic, each slot's
    /// sample indices and the reference's samples, and each job in work a median's histogram, a
    /// frame's samples and one transient copy of them — for its median, its seed's ratios, or a
    /// fit's residuals. A set every frame covers whole holds the same less the masks and the
    /// histograms. Pair by pair, each job holds its pair's mask, indices, both frames' samples and
    /// a histogram, and for a fitted gain the transient copy. A job never waits on a nested
    /// parallel loop, so no more of them are alive than there are workers. The ratio of medians
    /// needs no samples where the sources' medians describe the pixels combined, so nothing then.
    ///
    /// [`SAMPLE_LIMIT`]: crate::frame_store::stratified_samples::SAMPLE_LIMIT
    pub(crate) fn heap_bytes(self) -> usize {
        let Self {
            frame_count,
            desc,
            slots,
            normalization,
            coverage,
            threads,
        } = self;
        let samples: ArrayVec<usize, 3> = (0..slots.count())
            .map(|slot| StratifiedSamples::new(desc.size(), slots.mosaic(), slot).len())
            .collect();
        let most = samples.iter().copied().max().unwrap_or(0);
        let shared: usize = samples
            .iter()
            .map(|&count| count * (size_of::<usize>() + size_of::<f32>()))
            .sum();
        let mask = CommonDomain::heap_bytes(desc.pixel_count());
        let masks = mask * (1 + slots.mosaic().map_or(0, |_| slots.count()));
        let workers = threads.min(frame_count * slots.count());
        let sample = size_of::<f32>();
        let histogram = RadixMedian::HISTOGRAM_BYTES;
        // A pair's domain, its sample indices and the median's histogram; a global fit adds the two
        // frames' samples and the reference's copy its side keeps.
        let pair = mask + most * size_of::<usize>() + histogram;
        match (normalization, coverage) {
            (Normalization::None, _) => 0,
            (Normalization::Multiplicative, SetCoverage::Common) => masks + workers * histogram,
            (Normalization::Global, SetCoverage::Common) => {
                masks + shared + workers * (histogram + 2 * most * sample)
            }
            (Normalization::Multiplicative, SetCoverage::PerChannel) => workers * pair,
            (Normalization::Global, SetCoverage::PerChannel) => {
                workers * (pair + 3 * most * sample)
            }
        }
    }
}
