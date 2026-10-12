//! The body of the stacking entries: [`StackConfig::stack_paths`] from files and
//! [`StackConfig::stack_images`] from frames in memory, and the one combine every stack goes
//! through.

mod quantization;

use std::path::Path;

use crate::image::image_dimensions::ImageDimensions;
use crate::image::image_metadata::ImageMetadata;
use crate::image::linear_image::LinearImage;
use common::CancelToken;
use imaginarium::Buffer2;

use crate::combine::error::{StackConfigError, StackError};
use crate::combine::frame_cache::core::CacheCore;
use crate::combine::frame_cache::frame_weights::FrameWeights;
use crate::combine::frame_cache::sample::CombinedSample;
use crate::combine::frame_cache::sample_noise::SampleNoise;
use crate::combine::frame_cache::{CombineOutput, CombineRequest, FrameCache};
use crate::combine::stack::quantization::SourceSigmas;
use crate::combine::stack_config::{CombineMethod, StackConfig, Weighting};
use crate::frame::frame_quality::FrameQuality;
use crate::frame::frame_stats::FrameStats;
use crate::frame_store::stored_frame::StoredFrame;
use crate::frame_store::tier::Tier;
use crate::ingest::ingest_config::IngestConfig;
use crate::ingest::ingest_run::IngestRun;
use crate::math;
use crate::progress::progress_callback::ProgressCallback;
use crate::registration::resample::WarpResult;
use crate::stack_product::StackProduct;
use crate::stack_product::quality_planes::QualityPlanes;

/// One input frame for [`StackConfig::stack_images`], with the per-pixel frame quality a registered
/// frame carries.
///
/// Its coverage plane gates whether a warped sample has meaningful source support; its confidence
/// plane is an independent inverse-variance multiplier. A frame with no frame quality has full
/// support at unit confidence throughout. Plain `LinearImage`s convert with `.into()`; registered
/// frames must use [`StackFrame::registered`] so source-domain noise is captured before
/// interpolation.
#[derive(Debug, Clone)]
pub struct StackFrame {
    pub(crate) image: LinearImage,
    pub(crate) quality: FrameQuality<Buffer2<f32>>,
    pub(crate) source_stats: FrameStats,
}

impl StackFrame {
    /// Build a registered stack frame while preserving statistics from before interpolation.
    pub fn registered(source: &LinearImage, warped: WarpResult) -> Self {
        Self {
            source_stats: FrameStats::measure(source),
            image: warped.image,
            quality: FrameQuality::Planes {
                coverage: warped.coverage,
                confidence: warped.confidence,
            },
        }
    }
}

impl From<LinearImage> for StackFrame {
    fn from(image: LinearImage) -> Self {
        let source_stats = FrameStats::measure(&image);
        let quality = FrameQuality::for_unwarped(&image);
        Self {
            image,
            quality,
            source_stats,
        }
    }
}

/// The body of [`StackConfig::stack_paths`].
pub(crate) fn stack_paths<P: AsRef<Path> + Sync>(
    paths: &[P],
    config: &StackConfig,
    ingest: &IngestConfig,
    progress: ProgressCallback,
    cancel: CancelToken,
) -> Result<StackProduct, StackError> {
    // Files on disk carry no coverage, so the combine treats every pixel as fully covered.
    // `cancel` rides on the cache from construction, so the load loop polls it too.
    combine_cached(config, paths.len(), "paths", || {
        FrameCache::from_paths(paths, config, IngestRun::new(ingest, cancel), progress)
    })
}

/// The body of [`StackConfig::stack_images`].
pub(crate) fn stack_images(
    frames: Vec<StackFrame>,
    config: &StackConfig,
    progress: ProgressCallback,
    cancel: CancelToken,
) -> Result<StackProduct, StackError> {
    let frame_count = frames.len();
    combine_cached(config, frame_count, "memory", || {
        FrameCache::from_stack_frames(frames, config.normalization, progress, cancel)
    })
}

/// Combine frames produced by the shared frame store.
pub(crate) fn stack_stored_frames(
    frames: Vec<StoredFrame>,
    tier: Tier,
    dimensions: ImageDimensions,
    metadata: ImageMetadata,
    config: &StackConfig,
    progress: ProgressCallback,
    cancel: CancelToken,
) -> Result<StackProduct, StackError> {
    let frame_count = frames.len();
    combine_cached(config, frame_count, "frame store", || {
        FrameCache::from_stored_frames(
            frames,
            CacheCore {
                tier,
                dimensions,
                metadata,
                progress,
                cancel,
            },
            config.normalization,
        )
    })
}

/// The single gate every combine passes through: reject an empty frame set, validate the
/// configuration against the frame count, build the cache, log what the run turned out to be,
/// reduce, and decide whether the result is real.
///
/// Every entry point routes here — including `stack_cfa_master`, which builds a CFA cache of its
/// own — so config validation happens in exactly one place rather than being a convention each
/// entry point has to remember.
///
/// `build` is deferred rather than taken as a built cache so that validation runs before a
/// potentially long load, and so the cache — whose spilled planes hold their disk space — drops at
/// the end of this scope on every path.
pub(crate) fn combine_cached(
    config: &StackConfig,
    frame_count: usize,
    source: &'static str,
    build: impl FnOnce() -> Result<FrameCache, StackError>,
) -> Result<StackProduct, StackError> {
    if frame_count == 0 {
        return Err(StackError::NoFrames);
    }
    config.validate()?;
    validate_manual_weights(config, frame_count)?;

    let cache = build()?;
    // Logged after the load so the tier and quality facts come from the cache itself rather than
    // being restated at each entry point.
    tracing::info!(
        source,
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
    let weighted_combine = matches!(method, CombineMethod::Mean(_));
    let norms = cache.frame_norms.as_deref();
    let slots = cache.slots;
    let weights = if weighted_combine {
        FrameWeights::resolve(&config.weighting, stats(), norms, slots)?
    } else {
        None
    };

    // A median is not a linear combination, so it has no variance to report whatever the caller
    // asked for. Resolving here means the reducer never allocates a plane it would drop.
    let planes = config.quality.resolve(weighted_combine);
    let measure_quality = planes.weight || planes.inverse_variance || planes.dispersion;

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
                if measure_quality {
                    CombinedSample::median(value, samples.weights)
                } else {
                    CombinedSample::value_only(value)
                }
            })
        }
        CombineMethod::Mean(rejection) => {
            let noise = (rejection.measures_spread() || planes.inverse_variance)
                .then(|| SampleNoise::new(stats(), norms, slots));
            let request = CombineRequest {
                weights: weights.as_ref(),
                planes,
                min_survivors,
                noise: noise.as_ref(),
                slots,
            };
            let dispersion_correction = rejection.dispersion_correction();
            cache.process_chunked(request, move |samples, scratch| {
                let mut sample =
                    rejection.combine_mean(samples, min_survivors, scratch, measure_quality);
                // Survivors of a band scatter as a Gaussian truncated to it; a pixel too small to
                // reject from kept every sample, with nothing cut.
                if scratch.survivors.is_some() {
                    sample.dispersion *= dispersion_correction;
                }
                sample
            })
        }
    };

    finish_unless_cancelled(cache, combined, planes, quantization_sigma)
}

/// Assemble the product, or report the run as cancelled — the single exit both of
/// [`run_stacking`]'s paths take, so neither can hand back a partial stack as a whole one.
fn finish_unless_cancelled(
    cache: &FrameCache,
    combined: CombineOutput,
    planes: QualityPlanes,
    quantization_sigma: Option<f32>,
) -> Result<StackProduct, StackError> {
    if cache.core.cancel.is_cancelled() {
        return Err(StackError::Cancelled);
    }
    Ok(cache.finish_product(combined, planes, quantization_sigma))
}

#[cfg(feature = "bench")]
pub(crate) mod bench;
#[cfg(test)]
mod tests;
