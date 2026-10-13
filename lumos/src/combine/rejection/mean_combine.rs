//! [`MeanCombine`]: a pixel's samples rejected, then averaged.

use crate::combine::frame_cache::sample::{CombinedSample, PixelSamples, SampleQuality};
use crate::combine::rejection::Rejection;
use crate::combine::rejection::scratch_buffers::ScratchBuffers;
use crate::math::sum;

/// What a [`CombineMethod::Mean`] reduces each pixel to: its samples rejected by `rejection`, which
/// keeps at least `min_survivors` of them, then averaged by their weights, with `quality` reported
/// beside the value.
///
/// [`CombineMethod::Mean`]: crate::combine::stack_config::CombineMethod::Mean
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct MeanCombine {
    pub(crate) rejection: Rejection,
    pub(crate) min_survivors: usize,
    pub(crate) quality: SampleQuality,
}

impl MeanCombine {
    /// Reject outliers, then reduce the survivors to their weighted mean.
    ///
    /// Only the mean is weighted; the rejection decides survivors from the values alone. That
    /// matches what `ImageIntegration`, Siril and DSS do: rejection asks which samples disagree
    /// with the others, which is a question about the normalized values, not about how much each
    /// frame is trusted. It also keeps GESD a valid test: its critical values come from the
    /// t-distribution for `n` iid observations, and there is no weighted form of them.
    pub(crate) fn combine(
        &self,
        samples: &PixelSamples<'_>,
        scratch: &mut ScratchBuffers,
    ) -> CombinedSample {
        let values = &*samples.values;
        let weights = samples.weights;
        debug_assert_eq!(values.len(), weights.len());
        if matches!(self.rejection, Rejection::None) || values.len() <= self.min_survivors {
            scratch.survivors = None;
            let value = sum::weighted_mean_f32(values, weights);
            return match self.quality {
                SampleQuality::Measured => {
                    CombinedSample::from_survivors(value, samples, 0..values.len())
                }
                SampleQuality::ValueOnly => CombinedSample::value_only(value),
            };
        }

        scratch.sorted.fill(values);
        let window = self
            .rejection
            .surviving_window(scratch, samples.noise, self.min_survivors);
        let positions = &scratch.sorted.positions()[window.clone()];
        scratch.weights.clear();
        scratch
            .weights
            .extend(positions.iter().map(|&position| weights[position as usize]));
        let value =
            sum::weighted_mean_f32(&scratch.sorted.values()[window.clone()], &scratch.weights);
        let sample = match self.quality {
            SampleQuality::Measured => CombinedSample::from_survivors(
                value,
                samples,
                positions.iter().map(|&position| position as usize),
            ),
            SampleQuality::ValueOnly => CombinedSample::value_only(value),
        };
        scratch.survivors = Some(window);
        sample
    }
}
