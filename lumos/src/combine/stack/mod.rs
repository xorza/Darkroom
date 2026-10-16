//! The one combine every stack goes through: [`combine`] reduces the cached frames to the stacked
//! product, under the settings [`StackConfig::combine_cached`] validated and resolved.
//!
//! [`StackConfig::combine_cached`]: crate::combine::stack_config::StackConfig::combine_cached

pub(crate) mod mean_combine;
pub(crate) mod planned_combine;
mod quantization;

use crate::combine::error::StackError;
use crate::combine::frame_cache::frame_weights::FrameWeights;
use crate::combine::frame_cache::normalized_stats::NormalizedStats;
use crate::combine::frame_cache::sample_noise::SampleNoise;
use crate::combine::frame_cache::{CombineRequest, FrameCache};
use crate::combine::reducer::combined_sample::CombinedSample;
use crate::combine::reducer::sample_quality::SampleQuality;
use crate::combine::stack::mean_combine::MeanCombine;
use crate::combine::stack_config::combine::Combine;
use crate::combine::stack_config::combine_method::CombineMethod;
use crate::combine::stack_config::weighting::Weighting;
use crate::math;
use crate::stack_product::StackProduct;
use crate::stack_product::quality_planes::QualityPlanes;

/// Where the frames of a combine come from, for its log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FrameSource {
    /// Files decoded into the run's tier.
    Paths,
    /// Mosaic calibration frames decoded into the run's tier.
    CfaPaths,
    /// Frames the caller held in memory.
    Memory,
    /// Frames the pipeline stored.
    FrameStore,
}

/// The frames a combine takes: where they come from, and how many there are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FrameSet {
    pub(crate) source: FrameSource,
    pub(crate) count: usize,
}

/// The settings a combine runs by, resolved from its configuration: how each pixel combines, how
/// frames weigh, which planes it writes, and the fewest samples a pixel keeps.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CombinePlan<'a> {
    pub(crate) combine: Combine,
    pub(crate) weighting: &'a Weighting,
    pub(crate) quality: QualityPlanes,
    pub(crate) min_survivors: usize,
}

/// Warn when manual frame weights were given but the combine is a median, which has no weighted
/// form here — the weights would be silently dropped. A pixel the small-sample fallback combines by
/// its median drops them too, which the run report counts. Noise weighting is a role's policy
/// rather than a request, as [`StackConfig::light`](crate::StackConfig::light) carries it whatever
/// the method, so a median that leaves it unused says nothing.
fn warn_if_weights_ignored(method: CombineMethod, weighting: &Weighting) {
    if matches!(method, CombineMethod::Median) && matches!(weighting, Weighting::Manual(_)) {
        tracing::warn!(
            ?weighting,
            "frame weighting is ignored by the median combine; use a Mean method to apply weights",
        );
    }
}

/// Combine the cached frames into one stacked product.
///
/// Coverage gates a frame's contribution at a pixel while confidence divides its noise model; a
/// frame with neither plane contributes everywhere at unit confidence, which is what makes this the
/// single engine for calibration masters and registered light stacks alike.
///
/// # Errors
///
/// [`StackError::Cancelled`] if the cache's token was set. The chunk walk abandons the output
/// between chunks rather than unwinding, so a cancelled run still produces a `StackProduct` — one
/// holding zeros wherever it stopped. Returning that as an error is what keeps the partial image
/// from being mistaken for a stack; the alternative, handing it back and trusting each caller to
/// consult the token, was missed by every caller but two.
pub(crate) fn combine(
    cache: &FrameCache,
    plan: CombinePlan<'_>,
) -> Result<StackProduct, StackError> {
    let CombinePlan {
        combine: Combine { method, small_n },
        weighting,
        quality: requested,
        min_survivors,
    } = plan;
    let stats = || cache.frames.iter().map(|frame| &frame.source_stats);
    let frame_count = cache.frames.len();
    if small_n.falls_back(method, frame_count) {
        tracing::warn!(
            frame_count,
            min_frames = small_n.min_frames,
            "too few frames for the configured rejection; every pixel combines by the fallback",
        );
    }
    warn_if_weights_ignored(method, weighting);
    let norms = cache.frame_norms.as_deref();
    let slots = cache.slots;
    let weights = match method {
        CombineMethod::Mean(_) => FrameWeights::resolve(
            weighting,
            NormalizedStats {
                stats: stats(),
                norms,
                slots,
            },
        )?,
        CombineMethod::Median => None,
    };

    // A median is not a linear combination, so it has no variance to report whatever the caller
    // asked for. Resolving here means the reducer never allocates a plane it would drop.
    let planes = method.planes(requested);
    let quality = SampleQuality::of(planes);

    // A master states the step of its inputs: the largest any frame contributes once its
    // normalization scaled it, whatever each pixel's survivors were.
    let quantization_sigma = quantization::largest_step(stats(), norms);

    let combined = match method {
        CombineMethod::Median => {
            let request = CombineRequest {
                weights: None,
                planes,
                min_survivors,
                noise: None,
            };
            cache.process_chunked(request, |samples, _| {
                // Reorders the values in place, away from their weights: only the weights' sum,
                // which no order changes, is read after it.
                let value = math::statistics::median_mut(samples.values);
                match quality {
                    SampleQuality::Measured => CombinedSample::median(value, samples.weights),
                    SampleQuality::ValueOnly => CombinedSample::value_only(value),
                }
            })
        }
        CombineMethod::Mean(rejection) => {
            let noise = (rejection.measures_spread() || planes.inverse_variance).then(|| {
                SampleNoise::new(NormalizedStats {
                    stats: stats(),
                    norms,
                    slots,
                })
            });
            if rejection.uses_ccd_model()
                && let Some(index) = noise.as_ref().and_then(SampleNoise::first_without_gain)
            {
                return Err(StackError::UnknownGain { index });
            }
            let request = CombineRequest {
                weights: weights.as_ref(),
                planes,
                min_survivors,
                noise: noise.as_ref(),
            };
            let mean = MeanCombine {
                rejection,
                small_n,
                min_survivors,
                quality,
            };
            cache.process_chunked(request, mean.reducer())
        }
    };

    // Both reductions leave here, so neither can hand back a partial stack as a whole one.
    if cache.core.monitor.cancel.is_cancelled() {
        return Err(StackError::Cancelled);
    }
    Ok(cache.finish_product(combined, planes, quantization_sigma))
}

#[cfg(feature = "internals")]
pub(crate) mod bench;
#[cfg(test)]
mod tests;
