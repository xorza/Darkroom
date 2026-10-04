//! Unified stacking entry point.
//!
//! Provides `stack()` (from paths) and `stack_images()` (in-memory) as the main API
//! for image stacking operations; both take a [`ProgressCallback`].

mod quantization;

use std::path::Path;

use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::image_metadata::ImageMetadata;
use crate::io::image::linear::LinearImage;
use common::CancelToken;
use imaginarium::Buffer2;

use crate::combine::cache::core::{CacheCore, CacheTier};
use crate::combine::cache::frame_weights::FrameWeights;
use crate::combine::cache::sample::{CombinedSample, PixelSamples};
use crate::combine::cache::sample_noise::SampleNoise;
use crate::combine::cache::slots::Slots;
use crate::combine::cache::{CombineOutput, CombineRequest, FrameCache};
use crate::combine::config::{CombineMethod, StackConfig, Weighting};
use crate::combine::error::{StackConfigError, StackError};
use crate::combine::rejection::scratch_buffers::ScratchBuffers;
use crate::combine::stack::quantization::{MaxSigma, SourceSigmas};
use crate::frame_store::frame_quality::FrameQuality;
use crate::frame_store::frame_stats::FrameStats;
use crate::frame_store::stored_frame::StoredFrame;
use crate::ingest::ingest_run::IngestRun;
use crate::math;
use crate::progress::ProgressCallback;
use crate::registration::resample::WarpResult;
use crate::stack_product::StackProduct;
use crate::stack_product::quality_planes::QualityPlanes;

/// One input frame for [`stack_images`], with the per-pixel frame quality a registered frame
/// carries.
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

/// Stack multiple images from disk into a single result.
///
/// This is the main entry point for stacking frames stored as files. To stack
/// frames already held in memory, use [`stack_images`].
///
/// # Arguments
///
/// * `paths` - Paths to input images
/// * `config` - Stacking configuration
///
/// # Returns
///
/// A [`StackProduct`] whose coverage is the fraction of frames with geometric support at each
/// pixel. Its per-channel weight map describes the surviving samples; `variance` is
/// available for mean output and absent for median output.
///
/// # Errors
///
/// Returns an error if:
/// - No paths are provided
/// - The configuration is invalid or its manual-weight count doesn't match the paths
/// - Image loading fails
/// - Image dimensions don't match
/// - A decoded image contains a non-finite sample
/// - Cache directory creation fails (for disk-backed storage)
///
/// # Examples
///
/// Pass [`ProgressCallback::default()`] when you don't need progress reporting.
///
/// ```no_run
/// use common::CancelToken;
/// use lumos::{ProgressCallback, StackConfig, stack};
///
/// let paths = ["frame1.fits", "frame2.fits", "frame3.fits"];
/// let result = stack(&paths, &StackConfig::default(), ProgressCallback::default(), CancelToken::never())?;
/// let result = stack(&paths, &StackConfig::median(), ProgressCallback::default(), CancelToken::never())?;
/// # Ok::<(), lumos::StackError>(())
/// ```
pub fn stack<P: AsRef<Path> + Sync>(
    paths: &[P],
    config: &StackConfig,
    progress: ProgressCallback,
    cancel: CancelToken,
) -> Result<StackProduct, StackError> {
    // Files on disk carry no coverage, so the combine treats every pixel as fully covered.
    // `cancel` rides on the cache from construction, so the load loop polls it too.
    combine_cached(config, paths.len(), "paths", || {
        FrameCache::from_paths(
            paths,
            config,
            IngestRun::new(&config.ingest, cancel),
            progress,
        )
    })
}

/// Stack frames already held in memory into a single result.
///
/// The in-memory counterpart to [`stack`]: skips the disk round-trip when the caller already
/// owns the decoded frames (e.g. straight off calibration or warping). The frames are consumed.
/// Pass [`ProgressCallback::default()`] when you don't need progress reporting.
///
/// Each [`StackFrame`] may carry per-pixel `coverage` and `confidence`. Coverage gates inclusion;
/// confidence scales inverse-variance weight. Frames without either plane use full support and unit
/// confidence. Plain `LinearImage`s convert via `.into()` and [`StackFrame::registered`] converts a
/// source image plus its [`WarpResult`] without remeasuring noise after interpolation.
///
/// Returns an error when the configuration is invalid, manual-weight count doesn't match the frame
/// count, an image contains a non-finite sample, image/quality-plane dimensions differ, or
/// normalization is requested for registered frames with no common valid support.
pub fn stack_images(
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
    tier: CacheTier,
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
        method = ?config.method,
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

/// Warn when frame weighting was requested but the resolved combine is a median, which has no
/// weighted form here — the weights would be silently dropped. Fires both for an explicit `Median`
/// and for a method downgraded to its small-N fallback (see
/// [`SmallN::resolve`](crate::combine::config::SmallN::resolve)).
fn warn_if_weights_ignored(method: CombineMethod, weighting: &Weighting) {
    if matches!(method, CombineMethod::Median) && *weighting != Weighting::Equal {
        tracing::warn!(
            ?weighting,
            "frame weighting is ignored by the median combine; use a Mean method to apply weights",
        );
    }
}

/// Combine the cached frames into one stacked product.
///
/// Coverage gates a frame's contribution at a pixel while confidence scales its statistical
/// weight independently; a frame with neither plane contributes everywhere at unit confidence,
/// which is what makes this the single engine for calibration masters and registered light
/// stacks alike.
///
/// # Errors
///
/// [`StackError::Cancelled`] if the cache's token was set. The chunk walk abandons the output between
/// chunks rather than unwinding, so a cancelled run still produces a `StackProduct` — one holding
/// zeros wherever it stopped. Returning that as an error is what keeps the partial image from
/// being mistaken for a stack; the alternative, handing it back and trusting each caller to
/// consult the token, was missed by every caller but two.
pub(crate) fn run_stacking(
    cache: &FrameCache,
    config: &StackConfig,
) -> Result<StackProduct, StackError> {
    let stats = || cache.frames.iter().map(|frame| &frame.source_stats);
    let frame_count = cache.frames.len();
    let method = config.small_n.resolve(config.method, frame_count);
    warn_if_weights_ignored(method, &config.weighting);
    let weighted_combine = matches!(method, CombineMethod::Mean(_));
    let norms = cache.frame_norms.as_deref();
    let slots = Slots::new(
        cache.frames[0].source_stats.facts.cfa_type,
        cache.core.dimensions.channels(),
    );
    let weights = if weighted_combine {
        FrameWeights::resolve(&config.weighting, stats(), norms, slots)?
    } else {
        None
    };

    // A median is not a linear combination, so it has no variance to report whatever the caller
    // asked for. Resolving here means the reducer never allocates a plane it would drop.
    let planes = config.quality.resolve(weighted_combine);
    let measure_quality = planes.weight || planes.variance;

    let sigmas = SourceSigmas::measure(stats());
    let min_survivors = config.min_survivors;

    let (combined, quantization_sigma) = match method {
        CombineMethod::Median => {
            let sigma = sigmas.and_then(|sigmas| sigmas.combined_median(norms));
            let request = CombineRequest {
                weights: None,
                planes,
                min_survivors,
                noise: None,
                slots,
            };
            let combined = cache.process_chunked(request, |samples, _| {
                let count = samples.values.len();
                let value = math::statistics::median_mut(samples.values);
                if measure_quality {
                    CombinedSample::from_survivors(value, samples.weights, 0..count, None)
                } else {
                    CombinedSample::value_only(value, count)
                }
            });
            (combined, sigma)
        }
        CombineMethod::Mean(rejection) => {
            let noise = (rejection.measures_spread() || planes.variance)
                .then(|| SampleNoise::new(stats(), norms, slots));
            let request = CombineRequest {
                weights: weights.as_ref(),
                planes,
                min_survivors,
                noise: noise.as_ref(),
                slots,
            };
            let reduce = move |samples: PixelSamples<'_>, scratch: &mut ScratchBuffers| {
                rejection.combine_mean(samples, min_survivors, scratch, measure_quality)
            };
            match sigmas {
                Some(sigmas) => {
                    // Rejection and coverage keep a different set of frames at every pixel, so the
                    // master's figure is the least-reduced pixel's: seed with every frame and raise
                    // it wherever a pixel had fewer.
                    let all_frames = (0..slots.count())
                        .map(|slot| {
                            sigmas
                                .combined_mean(
                                    norms,
                                    slots.channel(slot),
                                    (0..frame_count).map(|frame| {
                                        let weight = weights
                                            .as_ref()
                                            .map_or(1.0, |weights| weights.weight(frame, slot));
                                        (frame, weight)
                                    }),
                                )
                                .expect("a validated stack has positive total weight")
                        })
                        .fold(0.0, f32::max);
                    let max_sigma = MaxSigma::seeded(all_frames);
                    let combined = cache.process_chunked(request, |samples, scratch| {
                        let PixelSamples {
                            frame_ids,
                            weights,
                            channel,
                            ..
                        } = samples;
                        let sample = reduce(samples, scratch);
                        if sample.survivor_count != frame_count {
                            let survivor =
                                |position: usize| (frame_ids[position] as usize, weights[position]);
                            max_sigma.record(match scratch.survivor_positions() {
                                Some(positions) => sigmas.combined_mean(
                                    norms,
                                    channel,
                                    positions
                                        .iter()
                                        .map(|&position| survivor(position as usize)),
                                ),
                                None => sigmas.combined_mean(
                                    norms,
                                    channel,
                                    (0..frame_ids.len()).map(survivor),
                                ),
                            });
                        }
                        sample
                    });
                    (combined, max_sigma.get())
                }
                None => (cache.process_chunked(request, reduce), None),
            }
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

#[cfg(test)]
mod tests;
