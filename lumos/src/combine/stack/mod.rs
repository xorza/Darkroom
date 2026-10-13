//! The one combine every stack goes through: [`combine_cached`] validates and loads, and
//! [`run_stacking`] reduces the frames to the stacked product.

mod quantization;
pub(crate) mod stack_frame;

use crate::combine::error::{StackConfigError, StackError};
use crate::combine::frame_cache::core::CacheCore;
use crate::combine::frame_cache::frame_weights::FrameWeights;
use crate::combine::frame_cache::normalized_stats::NormalizedStats;
use crate::combine::frame_cache::sample::{CombinedSample, SampleQuality};
use crate::combine::frame_cache::sample_noise::SampleNoise;
use crate::combine::frame_cache::{CombineRequest, FrameCache};
use crate::combine::rejection::mean_combine::MeanCombine;
use crate::combine::stack::quantization::SourceSigmas;
use crate::combine::stack_config::{CombineMethod, StackConfig, Weighting};
use crate::frame_store::stored_frame::StoredFrame;
use crate::math;
use crate::stack_product::StackProduct;

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

/// Combine `frames`, which the shared frame store produced, over `core`.
pub(crate) fn stack_stored_frames(
    frames: Vec<StoredFrame>,
    core: CacheCore,
    config: &StackConfig,
) -> Result<StackProduct, StackError> {
    let set = FrameSet {
        source: FrameSource::FrameStore,
        count: frames.len(),
    };
    combine_cached(config, set, || {
        FrameCache::from_stored_frames(frames, core, config.normalization)
    })
}

/// The single gate every combine passes through: reject an empty frame set, validate the
/// configuration against the frame count, build the cache, log what the run turned out to be,
/// reduce, and decide whether the result is real.
///
/// Every entry point routes here — including `MasterStack`, which builds a CFA cache of its
/// own — so config validation happens in exactly one place rather than being a convention each
/// entry point has to remember.
///
/// `build` is deferred rather than taken as a built cache so that validation runs before a
/// potentially long load, and so the cache — whose spilled planes hold their disk space — drops at
/// the end of this scope on every path.
pub(crate) fn combine_cached(
    config: &StackConfig,
    set: FrameSet,
    build: impl FnOnce() -> Result<FrameCache, StackError>,
) -> Result<StackProduct, StackError> {
    let FrameSet {
        source,
        count: frame_count,
    } = set;
    if frame_count == 0 {
        return Err(StackError::NoFrames);
    }
    config.validate()?;
    validate_manual_weights(config, frame_count)?;

    let cache = build()?;
    // Logged after the load so the tier and quality facts come from the cache itself rather than
    // being restated at each entry point.
    tracing::info!(
        ?source,
        frame_count,
        method = ?config.combine.method,
        weighting = ?config.weighting,
        normalization = ?config.normalization,
        disk_tier = cache.core.tier.spills(),
        // Not "warped": a frame carries these when a warp produced them *or* when its source
        // declared pixels with no measurement.
        frame_quality = cache
            .frames
            .iter()
            .any(|frame| !frame.quality.is_none()),
        "Combining frames"
    );

    run_stacking(&cache, config)
}

const fn validate_manual_weights(
    config: &StackConfig,
    frame_count: usize,
) -> Result<(), StackConfigError> {
    if let Weighting::Manual(ref w) = config.weighting
        && w.len() != frame_count
    {
        return Err(StackConfigError::ManualWeightCountMismatch {
            expected: frame_count,
            actual: w.len(),
        });
    }
    Ok(())
}

/// Warn when manual frame weights were given but the resolved combine is a median, which has no
/// weighted form here — the weights would be silently dropped. Fires both for an explicit `Median`
/// and for a method downgraded to its small-N fallback (see
/// [`SmallN::resolve`](crate::combine::stack_config::SmallN::resolve)). Noise weighting is a role's
/// policy rather than a request, as [`StackConfig::light`] carries it whatever the method, so a
/// median that leaves it unused says nothing.
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
pub(crate) fn run_stacking(
    cache: &FrameCache,
    config: &StackConfig,
) -> Result<StackProduct, StackError> {
    let stats = || cache.frames.iter().map(|frame| &frame.source_stats);
    let frame_count = cache.frames.len();
    let method = config
        .combine
        .small_n
        .resolve(config.combine.method, frame_count);
    warn_if_weights_ignored(method, &config.weighting);
    let norms = cache.frame_norms.as_deref();
    let slots = cache.slots;
    let weights = match method {
        CombineMethod::Mean(_) => FrameWeights::resolve(
            &config.weighting,
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
    let planes = config.quality.resolve(method);
    let quality = SampleQuality::of(planes);

    // A master states the step of its inputs: the largest any frame contributes once its
    // normalization scaled it, whatever each pixel's survivors were.
    let quantization_sigma =
        SourceSigmas::measure(stats()).and_then(|sigmas| sigmas.largest(norms));
    let min_survivors = config.min_survivors;

    let combined = match method {
        CombineMethod::Median => {
            let request = CombineRequest {
                weights: None,
                planes,
                min_survivors,
                noise: None,
                slots,
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
            let request = CombineRequest {
                weights: weights.as_ref(),
                planes,
                min_survivors,
                noise: noise.as_ref(),
                slots,
            };
            let mean = MeanCombine {
                rejection,
                min_survivors,
                quality,
            };
            let dispersion_correction = rejection.dispersion_correction();
            cache.process_chunked(request, move |samples, scratch| {
                let mut sample = mean.combine(&samples, scratch);
                // Survivors of a band scatter as a Gaussian truncated to it; a pixel too small to
                // reject from kept every sample, with nothing cut.
                if scratch.survivors.is_some() {
                    sample.dispersion *= dispersion_correction;
                }
                sample
            })
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
