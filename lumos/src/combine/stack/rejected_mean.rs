//! [`RejectedMean`]: a pixel's samples rejected, then averaged — or, at a pixel too few frames
//! reached for the rejection, combined by the small-sample fallback.

use crate::combine::reducer::combined_sample::CombinedSample;
use crate::combine::reducer::pixel_samples::PixelSamples;
use crate::combine::reducer::sample_quality::SampleQuality;
use crate::combine::rejection::Rejection;
use crate::combine::rejection::scratch_buffers::ScratchBuffers;
use crate::combine::stack_config::small_n::{Fallback, SmallN};
use crate::math::sum;

/// What a [`Combine::RejectedMean`](crate::Combine::RejectedMean) reduces each pixel to: its
/// samples rejected by `rejection`, which keeps at least `min_survivors` of them, then averaged by
/// their weights, with `quality` reported beside the value. A pixel with fewer samples than
/// `small_n` asks for is combined by its fallback instead: a rejection's statistics on so few
/// samples are not to be trusted, whatever the frame count of the stack.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct RejectedMean {
    pub(crate) rejection: Rejection,
    pub(crate) small_n: Option<SmallN>,
    pub(crate) min_survivors: usize,
    pub(crate) quality: SampleQuality,
}

impl RejectedMean {
    /// The reducer of each pixel, its constants resolved once: the band's corrections and the
    /// fallback below the samples the rejection needs.
    ///
    /// The quality planes of a pixel the band decided state the variance of the clipped mean, which
    /// choosing the survivors from the data makes `1/tv` times the survivors' as fixed
    /// ([`Rejection::band_variance`]), whether or not the band cut a sample: the inverse variance
    /// is multiplied by `tv`, and the dispersion, whose survivors scatter as a Gaussian truncated
    /// to the band, `σ²·tv`, by `1/tv²`. At ±2.5σ `tv` is 0.911, and ten frames' planes read 6%
    /// and 12.5% low without it; the correction is asymptotic and overstates them a little at so
    /// few, the conservative side.
    ///
    /// Only the mean is weighted; the rejection decides survivors from the values alone, as
    /// `ImageIntegration`, Siril and DSS do: which samples disagree is a question about the
    /// normalized values, not about how much each frame is trusted. It also keeps GESD a valid
    /// test: its critical values come from the t-distribution for `n` iid observations, and there
    /// is no weighted form of them.
    pub(crate) fn reducer(
        self,
    ) -> impl Fn(PixelSamples<'_>, &mut ScratchBuffers) -> CombinedSample + Sync + Copy {
        let Self {
            rejection,
            small_n,
            min_survivors,
            quality,
        } = self;
        let band_variance = rejection.band_variance().unwrap_or(1.0);
        let inverse_variance_correction = band_variance as f32;
        let dispersion_correction = (1.0 / (band_variance * band_variance)) as f32;
        move |samples, scratch| {
            let count = samples.values.len();
            debug_assert_eq!(count, samples.weights.len());
            if let Some(small_n) = small_n.filter(|small_n| small_n.falls_back(count)) {
                // One or two samples' median is their mean, whose variance is exact.
                let sample = match small_n.fallback {
                    Fallback::Median if count > 2 => {
                        CombinedSample::of_small_median(samples, quality)
                    }
                    Fallback::Median | Fallback::Mean => mean_of_all(&samples, quality),
                };
                return CombinedSample {
                    small_n_fallback: true,
                    ..sample
                };
            }
            if count <= min_survivors {
                return mean_of_all(&samples, quality);
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
                    sample.inverse_variance *= inverse_variance_correction;
                    sample.dispersion *= dispersion_correction;
                    sample
                }
                SampleQuality::ValueOnly => CombinedSample::value_only(value),
            }
        }
    }
}

/// The weighted mean of every sample of `samples`, with `quality` beside it: the reducer of a
/// [`Combine::Mean`](crate::Combine::Mean), and of a pixel no rejection takes a sample from.
pub(crate) fn mean_of_all(samples: &PixelSamples<'_>, quality: SampleQuality) -> CombinedSample {
    let value = sum::weighted_mean_f32(samples.values, samples.weights);
    match quality {
        SampleQuality::Measured => {
            CombinedSample::from_survivors(value, samples, 0..samples.values.len())
        }
        SampleQuality::ValueOnly => CombinedSample::value_only(value),
    }
}
