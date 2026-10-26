//! [`MeanCombine`]: a pixel's samples rejected, then averaged — or, at a pixel too few frames
//! reached for the rejection, combined by the small-sample fallback.

use crate::combine::reducer::combined_sample::CombinedSample;
use crate::combine::reducer::pixel_samples::PixelSamples;
use crate::combine::reducer::sample_quality::SampleQuality;
use crate::combine::rejection::Rejection;
use crate::combine::rejection::scratch_buffers::ScratchBuffers;
use crate::combine::stack_config::combine_method::CombineMethod;
use crate::combine::stack_config::small_n::SmallN;
use crate::math::sum;

/// What a [`CombineMethod::Mean`] reduces each pixel to: its samples rejected by `rejection`, which
/// keeps at least `min_survivors` of them, then averaged by their weights, with `quality` reported
/// beside the value. A pixel with fewer samples than `small_n` asks for is combined by its fallback
/// instead: a rejection's statistics on so few samples are not to be trusted, whatever the frame
/// count of the stack.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct MeanCombine {
    pub(crate) rejection: Rejection,
    pub(crate) small_n: SmallN,
    pub(crate) min_survivors: usize,
    pub(crate) quality: SampleQuality,
}

/// How a pixel too few frames reached is combined.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Fallback {
    /// The median, which has no linear-combination variance to report.
    Median,
    /// The plain weighted mean.
    Mean,
}

impl MeanCombine {
    /// The reducer of each pixel, its constants resolved once: the band's dispersion correction,
    /// and the fallback below the samples the rejection needs.
    ///
    /// Only the mean is weighted; the rejection decides survivors from the values alone. That
    /// matches what `ImageIntegration`, Siril and DSS do: rejection asks which samples disagree
    /// with the others, which is a question about the normalized values, not about how much each
    /// frame is trusted. It also keeps GESD a valid test: its critical values come from the
    /// t-distribution for `n` iid observations, and there is no weighted form of them.
    pub(crate) fn reducer(
        self,
    ) -> impl Fn(PixelSamples<'_>, &mut ScratchBuffers) -> CombinedSample + Sync + Copy {
        let Self {
            rejection,
            small_n,
            min_survivors,
            quality,
        } = self;
        let method = CombineMethod::Mean(rejection);
        let fallback_kind = match small_n.fallback {
            CombineMethod::Median => Fallback::Median,
            CombineMethod::Mean(_) => Fallback::Mean,
        };
        let dispersion_correction = rejection.dispersion_correction();
        move |samples, scratch| {
            let count = samples.values.len();
            debug_assert_eq!(count, samples.weights.len());
            let fallback = small_n.falls_back(method, count).then_some(fallback_kind);
            if fallback == Some(Fallback::Median) {
                return CombinedSample {
                    small_n_fallback: true,
                    ..CombinedSample::of_median(samples, quality)
                };
            }
            if rejection == Rejection::None || fallback.is_some() || count <= min_survivors {
                let value = sum::weighted_mean_f32(samples.values, samples.weights);
                let sample = match quality {
                    SampleQuality::Measured => {
                        CombinedSample::from_survivors(value, &samples, 0..count)
                    }
                    SampleQuality::ValueOnly => CombinedSample::value_only(value),
                };
                return CombinedSample {
                    small_n_fallback: fallback.is_some(),
                    ..sample
                };
            }

            scratch.sorted.fill(samples.values);
            let window = rejection.surviving_window(scratch, samples.noise, min_survivors);
            let poss = &scratch.sorted.poss()[window.clone()];
            scratch.weights.clear();
            scratch
                .weights
                .extend(poss.iter().map(|&pos| samples.weights[pos as usize]));
            let value = sum::weighted_mean_f32(&scratch.sorted.values()[window], &scratch.weights);
            match quality {
                SampleQuality::Measured => {
                    let mut sample = CombinedSample::from_survivors(
                        value,
                        &samples,
                        poss.iter().map(|&pos| pos as usize),
                    );
                    // Survivors of a band scatter as a Gaussian truncated to it.
                    sample.dispersion *= dispersion_correction;
                    sample
                }
                SampleQuality::ValueOnly => CombinedSample::value_only(value),
            }
        }
    }
}
