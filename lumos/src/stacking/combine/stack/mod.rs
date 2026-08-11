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

use crate::math;
use crate::stacking::combine::cache::sample::CombinedSample;
use crate::stacking::combine::cache::{CombineOutput, FrameCache, FrameCacheParams};
use crate::stacking::combine::config::{CombineMethod, StackConfig, Weighting};
use crate::stacking::combine::error::{Error, StackConfigError};
use crate::stacking::combine::normalization::FrameNorm;
use crate::stacking::combine::rejection::Rejection;
use crate::stacking::combine::stack::quantization::{MaxSigma, SourceSigmas};
use crate::stacking::frame_store::StoredFrame;
use crate::stacking::frame_store::frame_stats::FrameStats;
use crate::stacking::frame_store::spill::SpillDirectory;
use crate::stacking::frame_store::warp_quality::WarpQuality;
use crate::stacking::progress::ProgressCallback;
use crate::stacking::registration::resample::WarpResult;
use crate::stacking::stack_product::StackProduct;
use crate::stacking::stack_product::quality_planes::QualityPlanes;

/// One input frame for [`stack_images`], with the per-pixel warp quality a registered frame carries.
///
/// Its coverage plane gates whether a warped sample has meaningful source support; its confidence
/// plane is an independent inverse-variance multiplier. A frame with no warp quality has full
/// support at unit confidence throughout. Plain `LinearImage`s convert with `.into()`; registered
/// frames must use [`StackFrame::registered`] so source-domain noise is captured before
/// interpolation.
#[derive(Debug)]
pub struct StackFrame {
    pub(crate) image: LinearImage,
    pub(crate) quality: WarpQuality<Buffer2<f32>>,
    pub(crate) source_stats: FrameStats,
}

impl StackFrame {
    /// Build a registered stack frame while preserving statistics from before interpolation.
    pub fn registered(source: &LinearImage, warped: WarpResult) -> Self {
        Self {
            source_stats: FrameStats::measure(source),
            image: warped.image,
            quality: WarpQuality::Planes {
                coverage: warped.coverage,
                confidence: warped.confidence,
            },
        }
    }
}

impl From<LinearImage> for StackFrame {
    fn from(image: LinearImage) -> Self {
        let source_stats = FrameStats::measure(&image);
        Self {
            image,
            quality: WarpQuality::None,
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
/// pixel. Its per-channel weight map describes the surviving samples; `linear_variance` is
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
/// ```ignore
/// use lumos::{stack, StackConfig, ProgressCallback};
///
/// let result = stack(&paths, StackConfig::default(), ProgressCallback::default(), CancelToken::never())?;
/// let result = stack(&paths, StackConfig::median(), ProgressCallback::default(), CancelToken::never())?;
/// ```
pub fn stack<P: AsRef<Path> + Sync>(
    paths: &[P],
    config: StackConfig,
    progress: ProgressCallback,
    cancel: CancelToken,
) -> Result<StackProduct, Error> {
    // Files on disk carry no coverage, so the combine treats every pixel as fully covered.
    // `cancel` rides on the cache from construction, so the load loop polls it too.
    combine_cached(&config, paths.len(), "paths", || {
        FrameCache::from_paths(paths, &config.cache, config.normalization, progress, cancel)
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
    config: StackConfig,
    progress: ProgressCallback,
    cancel: CancelToken,
) -> Result<StackProduct, Error> {
    let frame_count = frames.len();
    combine_cached(&config, frame_count, "memory", || {
        FrameCache::from_stack_frames(
            frames,
            &config.cache,
            config.normalization,
            progress,
            cancel,
        )
    })
}

/// Combine frames produced by the shared frame store.
pub(crate) fn stack_stored_frames(
    frames: Vec<StoredFrame>,
    spill_directory: Option<SpillDirectory>,
    dimensions: ImageDimensions,
    metadata: ImageMetadata,
    config: StackConfig,
    progress: ProgressCallback,
    cancel: CancelToken,
) -> Result<StackProduct, Error> {
    let frame_count = frames.len();
    combine_cached(&config, frame_count, "frame store", || {
        FrameCache::from_stored_frames(
            frames,
            FrameCacheParams {
                spill_directory,
                dimensions,
                metadata,
                config: config.cache.clone(),
                normalization: config.normalization,
                progress,
                cancel,
            },
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
/// potentially long load, and so the cache — which owns the spill directory — drops at the end of
/// this scope on every path.
pub(crate) fn combine_cached(
    config: &StackConfig,
    frame_count: usize,
    source: &'static str,
    build: impl FnOnce() -> Result<FrameCache, Error>,
) -> Result<StackProduct, Error> {
    if frame_count == 0 {
        return Err(Error::NoFrames);
    }
    config.validate()?;
    validate_manual_weights(config, frame_count)?;

    let cache = build()?;
    // Logged after the load so the tier and warp-quality facts come from the cache itself rather
    // than being restated at each entry point.
    tracing::info!(
        source,
        frame_count,
        method = ?config.method,
        weighting = ?config.weighting,
        normalization = ?config.normalization,
        disk_tier = cache.core.spill_directory.is_some(),
        warp_quality = cache
            .frames
            .iter()
            .any(|frame| !frame.quality.is_none()),
        "Combining frames"
    );

    run_stacking(&cache, config)
}

/// Resolve weights from the weighting strategy and pre-computed channel stats.
///
/// Returns normalized weights (sum to 1.0) or `None` for equal weighting.
fn resolve_weights<'a>(
    weighting: &Weighting,
    stats: impl IntoIterator<Item = &'a FrameStats>,
    frame_norms: Option<&[FrameNorm]>,
) -> Option<Vec<f32>> {
    match weighting {
        Weighting::Equal => None,
        Weighting::Noise => {
            let mut stats = stats.into_iter().peekable();
            assert!(stats.peek().is_some(), "noise weighting requires frames");
            // Inverse variance of the frame *as combined*: normalization multiplies the frame by
            // `gain`, scaling its noise to `gain·σ`, so w = 1/(gain·σ)² — the "pscale²" term.
            // Without it a frame scaled up to match the reference is over-weighted by gain².
            let weights: Vec<f32> = stats
                .enumerate()
                .map(|(frame_idx, fs)| {
                    let sigma_sum: f32 = fs
                        .channels
                        .iter()
                        .enumerate()
                        .map(|(channel, c)| {
                            let gain =
                                frame_norms.map_or(1.0, |n| n[frame_idx].channels[channel].gain);
                            gain * math::statistics::mad_to_sigma(c.mad)
                        })
                        .sum();
                    let avg_sigma = sigma_sum / fs.channels.len() as f32;
                    if avg_sigma > f32::EPSILON {
                        1.0 / (avg_sigma * avg_sigma)
                    } else {
                        0.0
                    }
                })
                .collect();
            normalize_weights(&weights)
        }
        Weighting::Manual(w) => normalize_weights(w),
    }
}

fn validate_manual_weights(
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

/// Normalize weights to sum to 1.0. Returns `None` if the total cannot be normalized.
fn normalize_weights(weights: &[f32]) -> Option<Vec<f32>> {
    let sum: f32 = weights.iter().sum();
    if sum.is_finite() && sum > 0.0 {
        Some(weights.iter().map(|w| w / sum).collect())
    } else {
        None
    }
}

/// Warn when frame weighting was requested but the resolved combine is a median, which has no
/// weighted form here — the weights would be silently dropped. Fires both for an explicit `Median`
/// and for a method downgraded to its small-N fallback (see [`SmallN::resolve`]).
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
/// [`Error::Cancelled`] if the cache's token was set. The chunk walk abandons the output between
/// chunks rather than unwinding, so a cancelled run still produces a `StackProduct` — one holding
/// zeros wherever it stopped. Returning that as an error is what keeps the partial image from
/// being mistaken for a stack; the alternative, handing it back and trusting each caller to
/// consult the token, was missed by every caller but two.
pub(crate) fn run_stacking(
    cache: &FrameCache,
    config: &StackConfig,
) -> Result<StackProduct, Error> {
    // Normalization parameters are measured once, when the cache is built, and the combine reads
    // them from the cache rather than from `config`. Running a cache against a config that asks
    // for a different normalization would silently apply the one it was built with, so require
    // the two to agree instead.
    assert_eq!(
        cache.normalization, config.normalization,
        "cache was normalized as {:?} but the combine config asks for {:?}",
        cache.normalization, config.normalization
    );
    let stats = || cache.frames.iter().map(|frame| &frame.source_stats);
    let frame_count = cache.frames.len();
    let method = config.small_n.resolve(config.method, frame_count);
    warn_if_weights_ignored(method, &config.weighting);
    let weighted_combine = matches!(method, CombineMethod::Mean(_));
    let norms = cache.frame_norms.as_deref();
    let weights = weighted_combine
        .then(|| resolve_weights(&config.weighting, stats(), norms))
        .flatten();

    // A median is not a linear combination, so it has no variance factor to report whatever the
    // caller asked for. Resolving here means the reducer never allocates a plane it would drop.
    let planes = config.quality.resolve(weighted_combine);
    let measure_quality = planes.weight || planes.variance;

    let source_sigmas = SourceSigmas::measure(stats());
    // Propagating quantization noise through rejection needs each surviving sample's *frame*
    // index, to reach that frame's source sigma and normalization gain. Under partial coverage
    // the reducer sees a compacted subset whose indices no longer name frames, so the tracking
    // is limited to frame sets that carry no coverage — every calibration master, and every
    // light stack loaded straight from disk.
    let frame_indices_are_stable = cache.frames.iter().all(|frame| frame.quality.is_none());
    let sigmas = source_sigmas.filter(|_| frame_indices_are_stable);

    let (combined, quantization_sigma) = match method {
        CombineMethod::Median => {
            let sigma = sigmas.and_then(|sigmas| sigmas.combined_median(norms));
            let combined = cache.process_chunked(None, norms, planes, |values, w, _| {
                let value = math::statistics::median_f32_mut(values);
                if measure_quality {
                    CombinedSample::from_all(value, w)
                } else {
                    CombinedSample::value_only(value, values.len())
                }
            });
            (combined, sigma)
        }
        // Winsorization replaces samples with order statistics, so it has no fixed linear
        // coefficient set from which to propagate quantization variance.
        CombineMethod::Mean(rejection @ Rejection::Winsorized(_)) => {
            let sigma = sigmas.and_then(|sigmas| sigmas.conservative(norms));
            let combined = cache.process_chunked(
                weights.as_deref(),
                norms,
                planes,
                move |values, w, scratch| {
                    rejection.combine_mean(values, w, scratch, measure_quality)
                },
            );
            (combined, sigma)
        }
        CombineMethod::Mean(rejection) => {
            let Some(sigmas) = sigmas else {
                let combined = cache.process_chunked(
                    weights.as_deref(),
                    norms,
                    planes,
                    move |values, w, scratch| {
                        rejection.combine_mean(values, w, scratch, measure_quality)
                    },
                );
                return finish_unless_cancelled(cache, combined, planes, None);
            };
            // Rejection keeps a different survivor set at every pixel, so the master's floor is
            // the least-reduced pixel: seed with "nothing rejected" and raise it wherever a
            // pixel lost frames.
            let all_survivors = sigmas
                .combined_mean(weights.as_deref(), norms, 0..frame_count)
                .expect("a validated stack has positive total weight");
            let max_sigma = MaxSigma::seeded(all_survivors);
            let frame_weights = weights.as_deref();
            let combined =
                cache.process_chunked(weights.as_deref(), norms, planes, |values, w, scratch| {
                    let sample = rejection.combine_mean(values, w, scratch, measure_quality);
                    if sample.survivor_count != frame_count {
                        max_sigma.record(sigmas.combined_mean(
                            frame_weights,
                            norms,
                            scratch.indices[..sample.survivor_count].iter().copied(),
                        ));
                    }
                    sample
                });
            (combined, max_sigma.get())
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
) -> Result<StackProduct, Error> {
    if cache.core.cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    Ok(cache.finish_product(combined, planes, quantization_sigma))
}

#[cfg(test)]
mod tests;
