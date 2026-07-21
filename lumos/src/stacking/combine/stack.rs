//! Unified stacking entry point.
//!
//! Provides `stack()` (from paths) and `stack_images()` (in-memory) as the main API
//! for image stacking operations; both take a [`ProgressCallback`].

use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::io::image::cfa::CfaImage;
use crate::io::image::linear::LinearImage;
use crate::io::image::{ImageDimensions, ImageMetadata};
use common::CancelToken;
use imaginarium::Buffer2;

use crate::math;
use crate::stacking::combine::cache::{
    CfaCache, CombinedSample, LightCache, StoredLightCacheParams,
};
use crate::stacking::combine::config::{CombineMethod, StackConfig, Weighting};
use crate::stacking::combine::error::{Error, StackConfigError};
use crate::stacking::combine::normalization::{FrameNorm, compute_frame_norms};
use crate::stacking::combine::rejection::Rejection;
use crate::stacking::frame_store::{
    FrameStats, SpillDirectory, StoredLightFrame, compute_frame_stats,
};
use crate::stacking::product::StackProduct;
use crate::stacking::progress::ProgressCallback;
use crate::stacking::registration::resample::WarpResult;

/// One input frame for [`stack_images`] with optional per-pixel support and confidence.
///
/// `coverage` gates whether a warped sample has meaningful source support. `confidence` is an
/// independent inverse-variance multiplier. `None` means full support or unit confidence,
/// respectively. Plain `LinearImage`s convert with `.into()`; registered frames must use
/// [`StackFrame::registered`] so source-domain noise is captured before interpolation.
#[derive(Debug)]
pub struct StackFrame {
    pub(crate) image: LinearImage,
    pub(crate) coverage: Option<Buffer2<f32>>,
    pub(crate) confidence: Option<Buffer2<f32>>,
    pub(crate) source_stats: FrameStats,
}

impl StackFrame {
    /// Build a registered stack frame while preserving statistics from before interpolation.
    pub fn registered(source: &LinearImage, warped: WarpResult) -> Self {
        Self {
            source_stats: compute_frame_stats(source),
            image: warped.image,
            coverage: Some(warped.coverage),
            confidence: Some(warped.confidence),
        }
    }
}

impl From<LinearImage> for StackFrame {
    fn from(image: LinearImage) -> Self {
        let source_stats = compute_frame_stats(&image);
        Self {
            image,
            coverage: None,
            confidence: None,
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
    if paths.is_empty() {
        return Err(Error::NoFrames);
    }

    config.validate()?;
    validate_manual_weights(&config, paths.len())?;

    tracing::info!(
        method = ?config.method,
        weighting = ?config.weighting,
        normalization = ?config.normalization,
        frame_count = paths.len(),
        "Starting unified stack (from paths)"
    );

    // Files on disk carry no coverage, so this is a `LightCache` with `coverage: None` — the
    // weighted combine then treats every pixel as fully covered (identical to a plain stack).
    // `cancel` rides on the cache from construction, so the load loop polls it too.
    let cache =
        LightCache::from_paths(paths, &config.cache, config.normalization, progress, cancel)?;
    // The disk cache (if any) is removed when `cache` drops via `CacheCore`'s `Drop`.
    let result = run_stacking_weighted(&cache, &config);
    if cache.core.cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    Ok(result)
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
    if frames.is_empty() {
        return Err(Error::NoFrames);
    }

    config.validate()?;
    validate_manual_weights(&config, frames.len())?;

    tracing::info!(
        method = ?config.method,
        weighting = ?config.weighting,
        normalization = ?config.normalization,
        frame_count = frames.len(),
        warp_quality_weighted = frames
            .iter()
            .any(|f| f.coverage.is_some() || f.confidence.is_some()),
        "Starting unified stack (in memory)"
    );

    let cache = LightCache::from_stack_frames(
        frames,
        &config.cache,
        config.normalization,
        progress,
        cancel,
    )?;
    // In-memory only (no disk cache), but `cache` drops cleanly via `CacheCore`'s `Drop` regardless.
    let result = run_stacking_weighted(&cache, &config);
    if cache.core.cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    Ok(result)
}

/// Combine frames produced by the shared frame store.
pub(crate) fn stack_stored_frames(
    frames: Vec<StoredLightFrame>,
    spill_directory: Option<SpillDirectory>,
    dimensions: ImageDimensions,
    metadata: ImageMetadata,
    config: StackConfig,
    progress: ProgressCallback,
    cancel: CancelToken,
) -> Result<StackProduct, Error> {
    if frames.is_empty() {
        return Err(Error::NoFrames);
    }
    config.validate()?;
    validate_manual_weights(&config, frames.len())?;

    tracing::info!(
        method = ?config.method,
        weighting = ?config.weighting,
        normalization = ?config.normalization,
        frame_count = frames.len(),
        disk_tier = spill_directory.is_some(),
        "Starting unified stack (pre-tiered frames)"
    );

    let cache = LightCache::from_stored_frames(
        frames,
        StoredLightCacheParams {
            spill_directory,
            dimensions,
            metadata,
            config: config.cache.clone(),
            normalization: config.normalization,
            progress,
            cancel,
        },
    )?;
    let result = run_stacking_weighted(&cache, &config);
    if cache.core.cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    Ok(result)
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

fn source_quantization_sigmas(stats: &[FrameStats]) -> Option<Vec<f32>> {
    stats
        .iter()
        .map(|stats| {
            stats
                .quantization_sigma
                .filter(|sigma| sigma.is_finite() && *sigma > 0.0)
        })
        .collect()
}

fn combined_mean_quantization_sigma(
    source_sigmas: &[f32],
    weights: Option<&[f32]>,
    frame_norms: Option<&[FrameNorm]>,
    survivor_indices: impl IntoIterator<Item = usize>,
) -> Option<f32> {
    let mut total_weight = 0.0f32;
    let mut variance = 0.0f32;
    for index in survivor_indices {
        let weight = weights.map_or(1.0, |values| values[index]);
        let gain = frame_norms.map_or(1.0, |norms| norms[index].channels[0].gain);
        total_weight += weight;
        variance += (weight * gain * source_sigmas[index]).powi(2);
    }
    (total_weight > 0.0).then(|| variance.sqrt() / total_weight)
}

fn conservative_quantization_sigma(
    source_sigmas: &[f32],
    frame_norms: Option<&[FrameNorm]>,
) -> Option<f32> {
    source_sigmas
        .iter()
        .enumerate()
        .map(|(index, sigma)| {
            frame_norms.map_or(*sigma, |norms| norms[index].channels[0].gain.abs() * sigma)
        })
        .reduce(f32::max)
}

fn combined_median_quantization_sigma(
    source_sigmas: &[f32],
    frame_norms: Option<&[FrameNorm]>,
) -> Option<f32> {
    let conservative = conservative_quantization_sigma(source_sigmas, frame_norms)?;
    if frame_norms.is_some() {
        return Some(conservative);
    }
    let (&source_sigma, rest) = source_sigmas.split_first()?;
    if rest
        .iter()
        .any(|sigma| sigma.to_bits() != source_sigma.to_bits())
    {
        return Some(conservative);
    }
    let n = source_sigmas.len() as f32;
    let factor = if source_sigmas.len().is_multiple_of(2) {
        (3.0 * n / ((n + 1.0) * (n + 2.0))).sqrt()
    } else {
        (3.0 / (n + 2.0)).sqrt()
    };
    Some(source_sigma * factor)
}

fn record_max_sigma(max_sigma_bits: &AtomicU32, sigma: Option<f32>) {
    if let Some(sigma) = sigma {
        let bits = sigma.to_bits();
        if bits > max_sigma_bits.load(Ordering::Relaxed) {
            max_sigma_bits.fetch_max(bits, Ordering::Relaxed);
        }
    }
}

fn tracked_sigma(max_sigma_bits: &AtomicU32) -> Option<f32> {
    let bits = max_sigma_bits.load(Ordering::Relaxed);
    (bits != 0).then(|| f32::from_bits(bits))
}

pub(crate) fn run_stacking(cache: &CfaCache, config: &StackConfig) -> CfaImage {
    let stats = &cache.frame_stats;
    let method = config.small_n.resolve(config.method, stats.len());
    warn_if_weights_ignored(method, &config.weighting);
    let frame_norms = compute_frame_norms(stats, config.normalization);
    let weights = resolve_weights(&config.weighting, stats, frame_norms.as_deref());
    let norms = frame_norms.as_deref();
    let source_sigmas = source_quantization_sigmas(stats);

    let (pixels, quantization_sigma) = match method {
        CombineMethod::Median => {
            let sigma = source_sigmas
                .as_deref()
                .and_then(|sigmas| combined_median_quantization_sigma(sigmas, norms));
            let pixels = cache.process_chunked(None, norms, |values, _, _| {
                math::statistics::median_f32_mut(values)
            });
            (pixels, sigma)
        }
        CombineMethod::Mean(Rejection::None) => {
            let sigma = source_sigmas.as_deref().and_then(|sigmas| {
                combined_mean_quantization_sigma(sigmas, weights.as_deref(), norms, 0..stats.len())
            });
            let pixels =
                cache.process_chunked(weights.as_deref(), norms, |values, weights, scratch| {
                    Rejection::None.combine_mean(values, weights, scratch)
                });
            (pixels, sigma)
        }
        // Winsorization replaces samples with order statistics, so it has no fixed linear
        // coefficient set from which to propagate quantization variance.
        CombineMethod::Mean(rejection @ Rejection::Winsorized(_)) => {
            let sigma = source_sigmas
                .as_deref()
                .and_then(|sigmas| conservative_quantization_sigma(sigmas, norms));
            let pixels = cache.process_chunked(
                weights.as_deref(),
                norms,
                move |values, weights, scratch| rejection.combine_mean(values, weights, scratch),
            );
            (pixels, sigma)
        }
        CombineMethod::Mean(rejection) => {
            if let Some(source_sigmas) = source_sigmas.as_deref() {
                let all_survivors_sigma = combined_mean_quantization_sigma(
                    source_sigmas,
                    weights.as_deref(),
                    norms,
                    0..stats.len(),
                )
                .expect("a validated CFA stack has positive total weight");
                let max_sigma_bits = AtomicU32::new(all_survivors_sigma.to_bits());
                let pixels =
                    cache.process_chunked(weights.as_deref(), norms, |values, weights, scratch| {
                        let sample =
                            rejection.combine_mean_with_survivors(values, weights, scratch);
                        if sample.survivor_count != source_sigmas.len() {
                            record_max_sigma(
                                &max_sigma_bits,
                                combined_mean_quantization_sigma(
                                    source_sigmas,
                                    weights,
                                    norms,
                                    scratch.indices[..sample.survivor_count].iter().copied(),
                                ),
                            );
                        }
                        sample.value
                    });
                (pixels, tracked_sigma(&max_sigma_bits))
            } else {
                let pixels = cache.process_chunked(
                    weights.as_deref(),
                    norms,
                    move |values, weights, scratch| {
                        rejection.combine_mean(values, weights, scratch)
                    },
                );
                (pixels, None)
            }
        }
    };
    CfaImage {
        data: pixels.into_l(),
        metadata: cache.core.metadata.clone(),
        quantization_sigma,
    }
}

/// Coverage-weighted counterpart to [`run_stacking`] for a [`LightCache`]: identical combine math,
/// but each frame contributes only where it covers (`process_chunked_weighted`).
pub(crate) fn run_stacking_weighted(cache: &LightCache, config: &StackConfig) -> StackProduct {
    let method = config.small_n.resolve(config.method, cache.frames.len());
    warn_if_weights_ignored(method, &config.weighting);
    let weighted_combine = matches!(method, CombineMethod::Mean(_));
    let frame_norms = cache.frame_norms.as_deref();
    let weights = weighted_combine
        .then(|| {
            resolve_weights(
                &config.weighting,
                cache.frames.iter().map(|frame| &frame.source_stats),
                frame_norms,
            )
        })
        .flatten();

    let combined = match method {
        CombineMethod::Median => {
            let mut combined = cache.process_chunked_weighted(None, frame_norms, |values, w, _| {
                CombinedSample::from_all(math::statistics::median_f32_mut(values), w)
            });
            combined.linear_variance = None;
            combined
        }
        CombineMethod::Mean(rejection) => cache.process_chunked_weighted(
            weights.as_deref(),
            frame_norms,
            move |values, w, scratch| rejection.combine_mean_with_quality(values, w, scratch),
        ),
    };

    cache.finish_product(combined)
}

#[cfg(test)]
mod tests {
    use arrayvec::ArrayVec;

    use crate::io::image::ImageDimensions;
    use crate::io::image::cfa::{CfaImage, CfaType};
    use crate::io::image::linear::LinearImage;
    use crate::io::image::pixel_data::PixelData;
    use crate::math::statistics::ChannelStats;
    use crate::stacking::combine::cache::CacheCore;
    use crate::stacking::combine::cache::tests::make_test_cache;
    use crate::stacking::combine::cache_config::CacheConfig;
    use crate::stacking::combine::config::{Normalization, SmallN};
    use crate::stacking::combine::normalization;
    use crate::stacking::combine::normalization::{ChannelNorm, FrameNorm};
    use crate::stacking::combine::rejection::{PercentileClipConfig, Rejection};
    use crate::stacking::combine::stack::*;
    use crate::stacking::frame_store::{compute_frame_stats, frame_from_memory, store_light_frame};
    use crate::stacking::product::QualityMap;
    use crate::stacking::registration::config::{self, InterpolationMethod};
    use crate::stacking::registration::resample;
    use crate::stacking::registration::transform::{Transform, WarpTransform};
    use crate::testing::ScratchDirectory;
    use std::path::PathBuf;

    fn stack_frame(
        image: LinearImage,
        coverage: Option<Buffer2<f32>>,
        confidence: Option<Buffer2<f32>>,
    ) -> StackFrame {
        let mut frame = StackFrame::from(image);
        frame.coverage = coverage;
        frame.confidence = confidence;
        frame
    }

    fn make_cfa_stack_cache(
        frame_pixels: Vec<Vec<f32>>,
        source_sigmas: &[f32],
        dimensions: ImageDimensions,
    ) -> CfaCache {
        assert_eq!(frame_pixels.len(), source_sigmas.len());
        let images: Vec<CfaImage> = frame_pixels
            .into_iter()
            .zip(source_sigmas)
            .map(|(pixels, &sigma)| {
                let mut image = crate::testing::make_cfa(
                    dimensions.width(),
                    dimensions.height(),
                    pixels,
                    CfaType::Mono,
                );
                image.quantization_sigma = Some(sigma);
                image
            })
            .collect();
        let metadata = images[0].metadata.clone();
        let frame_stats = images.iter().map(compute_frame_stats).collect();
        let frames = images.into_iter().map(frame_from_memory).collect();
        CfaCache {
            frames,
            frame_stats,
            core: CacheCore {
                spill_directory: None,
                dimensions,
                metadata,
                config: CacheConfig::default(),
                progress: ProgressCallback::default(),
                cancel: CancelToken::never(),
            },
        }
    }

    #[test]
    fn quantization_helpers_follow_per_frame_coefficients_and_median_order_statistics() {
        let source_sigma = 0.01;
        let equal_sigmas = [source_sigma; 4];
        let equal_mean = combined_mean_quantization_sigma(&equal_sigmas, None, None, 0..4).unwrap();
        assert!(
            (equal_mean - 0.005).abs() < f32::EPSILON,
            "four-frame equal mean: σ/√4 = 0.005, got {equal_mean}"
        );

        let source_sigmas = [0.01, 0.02];
        let weighted =
            combined_mean_quantization_sigma(&source_sigmas, Some(&[0.75, 0.25]), None, 0..2)
                .unwrap();
        let expected_weighted = ((0.75f32 * 0.01).powi(2) + (0.25f32 * 0.02).powi(2)).sqrt();
        assert!(
            (weighted - expected_weighted).abs() < f32::EPSILON,
            "weighted unequal-source mean: expected {expected_weighted}, got {weighted}"
        );

        let median_two = combined_median_quantization_sigma(&[source_sigma; 2], None).unwrap();
        let median_three = combined_median_quantization_sigma(&[source_sigma; 3], None).unwrap();
        assert!(
            (median_two - source_sigma / 2.0f32.sqrt()).abs() < f32::EPSILON,
            "two-sample uniform median averages both samples: σ/√2, got {median_two}"
        );
        assert!(
            (median_three - source_sigma * (3.0f32 / 5.0).sqrt()).abs() < f32::EPSILON,
            "three-sample uniform median order statistic: σ·√(3/5), got {median_three}"
        );
        assert!(
            (combined_median_quantization_sigma(&source_sigmas, None).unwrap() - 0.02).abs()
                < f32::EPSILON,
            "unequal uniform source widths must retain the conservative largest σ"
        );
    }

    #[test]
    fn cfa_stack_quantization_uses_normalization_and_actual_rejection_survivors() {
        let dimensions = ImageDimensions::new((2, 1), 1);
        let mut normalized_cache =
            make_cfa_stack_cache(vec![vec![0.4; 2], vec![0.2; 2]], &[0.01, 0.02], dimensions);
        normalized_cache.frame_stats[0].channels[0] = ChannelStats {
            median: 0.4,
            mad: 0.01,
        };
        normalized_cache.frame_stats[1].channels[0] = ChannelStats {
            median: 0.2,
            mad: 0.02,
        };
        let normalized = run_stacking(
            &normalized_cache,
            &StackConfig {
                method: CombineMethod::Mean(Rejection::None),
                weighting: Weighting::Manual(vec![0.25, 0.75]),
                normalization: Normalization::Multiplicative,
                small_n: SmallN::none(),
                ..Default::default()
            },
        );
        let expected_normalized_sigma =
            ((0.25f32 * 0.01).powi(2) + (0.75f32 * 2.0 * 0.02).powi(2)).sqrt();
        assert_eq!(normalized.data.to_vec(), vec![0.4; 2]);
        assert!(
            (normalized.quantization_sigma.unwrap() - expected_normalized_sigma).abs()
                < f32::EPSILON,
            "weighted normalized σ must use each frame's own source σ and gain"
        );

        let median_cache = make_cfa_stack_cache(
            vec![vec![0.3; 2], vec![0.4; 2], vec![0.5; 2]],
            &[0.01; 3],
            dimensions,
        );
        let median = run_stacking(&median_cache, &StackConfig::median());
        let expected_median_sigma = 0.01 * (3.0f32 / 5.0).sqrt();
        assert_eq!(median.data.to_vec(), vec![0.4; 2]);
        assert!(
            (median.quantization_sigma.unwrap() - expected_median_sigma).abs() < f32::EPSILON,
            "an equal-source three-frame median must use the exact uniform order statistic"
        );

        let winsorized_cache =
            make_cfa_stack_cache(vec![vec![0.3; 2], vec![0.5; 2]], &[0.01, 0.02], dimensions);
        let winsorized = run_stacking(&winsorized_cache, &StackConfig::winsorized(2.5));
        assert!(
            (winsorized.quantization_sigma.unwrap() - 0.02).abs() < f32::EPSILON,
            "nonlinear unequal-source combines must retain the conservative largest σ"
        );

        let rejection_cache = make_cfa_stack_cache(
            (0..8)
                .map(|frame| vec![[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 100.0][frame], 1.0])
                .collect(),
            &[0.01; 8],
            dimensions,
        );
        let rejected = run_stacking(
            &rejection_cache,
            &StackConfig {
                method: CombineMethod::Mean(Rejection::sigma_clip(2.0)),
                small_n: SmallN::none(),
                ..Default::default()
            },
        );
        assert_eq!(rejected.data.to_vec(), vec![4.0, 1.0]);
        let expected_rejected_sigma = 0.01 / 7.0f32.sqrt();
        assert!(
            (rejected.quantization_sigma.unwrap() - expected_rejected_sigma).abs() < f32::EPSILON,
            "the global CFA floor must use the least-reduced pixel's seven survivors"
        );
    }

    /// The load-bearing guarantee for memory-aware stacking: spilling frames to disk (mmap) and
    /// combining must be **bit-identical** to the all-RAM combine — same frames, same math, only the
    /// plane storage differs. Exercises σ-clip rejection + noise weighting + global norm + a partial
    /// coverage map (so the coverage spill round-trips too).
    #[test]
    fn disk_tier_output_is_bit_identical_to_memory_tier() {
        let (w, h, n) = (40usize, 30usize, 12usize);
        let dims = ImageDimensions::new((w, h), 1);
        let make_frame = |f: usize| -> StackFrame {
            let mut px = vec![0.0f32; w * h];
            for (i, p) in px.iter_mut().enumerate() {
                let hash = (i as u32).wrapping_mul(2654435761) ^ (f as u32).wrapping_mul(40503);
                *p = 0.2 + (f as f32) * 0.01 + (hash as f32 / u32::MAX as f32 - 0.5) * 0.02;
            }
            px[(f * 7) % (w * h)] = 0.95; // an outlier so rejection actually fires
            let image = LinearImage::from_planar_channels(dims, [px]);
            // Every other frame gets a partial coverage map (warped-border emulation).
            let coverage = f.is_multiple_of(2).then(|| {
                let mut c = vec![1.0f32; w * h];
                c[0] = 0.0;
                Buffer2::new(w, h, c)
            });
            let confidence = coverage.clone();
            stack_frame(image, coverage, confidence)
        };
        let frames: Vec<StackFrame> = (0..n).map(make_frame).collect();
        let config = StackConfig::light();

        let ram = stack_images(
            frames
                .iter()
                .map(|f| {
                    let mut frame =
                        stack_frame(f.image.clone(), f.coverage.clone(), f.confidence.clone());
                    frame.source_stats = f.source_stats.clone();
                    frame
                })
                .collect(),
            config.clone(),
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .unwrap();

        let scratch = ScratchDirectory::new("lumos_tier_test");
        let spill_directory = SpillDirectory::create(scratch.join("cache"), false).unwrap();
        let metadata = frames[0].image.metadata.clone();
        let stored = frames
            .into_iter()
            .enumerate()
            .map(|(i, f)| {
                store_light_frame(
                    &spill_directory.path,
                    &format!("f{i}"),
                    f.image,
                    f.coverage,
                    f.confidence,
                    f.source_stats,
                )
                .unwrap()
            })
            .collect();
        let disk = stack_stored_frames(
            stored,
            Some(spill_directory),
            dims,
            metadata,
            config,
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .unwrap();

        let bits = |b: &Buffer2<f32>| b.pixels().iter().map(|x| x.to_bits()).collect::<Vec<_>>();
        assert_eq!(
            bits(ram.image.channel(0)),
            bits(disk.image.channel(0)),
            "stacked image differs between RAM and disk tiers"
        );
        assert_eq!(
            bits(&ram.coverage),
            bits(&disk.coverage),
            "coverage differs"
        );
        let ram_linear_variance = ram.linear_variance.as_ref().unwrap();
        let disk_linear_variance = disk.linear_variance.as_ref().unwrap();
        for channel in 0..ram.image.channels() {
            assert_eq!(
                bits(ram.weight.channel(channel)),
                bits(disk.weight.channel(channel)),
                "weight channel {channel} differs"
            );
            assert_eq!(
                bits(ram_linear_variance.channel(channel)),
                bits(disk_linear_variance.channel(channel)),
                "variance channel {channel} differs"
            );
        }
    }

    #[test]
    fn mapped_frames_reject_nonfinite_samples_before_combining() {
        let dimensions = ImageDimensions::new((2, 1), 3);
        let finite = LinearImage::from_planar_channels(
            dimensions,
            [vec![1.0; 2], vec![1.0; 2], vec![1.0; 2]],
        );
        let source_stats = compute_frame_stats(&finite);
        let invalid = LinearImage::from_planar_channels(
            dimensions,
            [vec![1.0; 2], vec![2.0; 2], vec![3.0, f32::NEG_INFINITY]],
        );
        let scratch = ScratchDirectory::new("lumos_nonfinite_mapped_frame");
        let spill_directory = SpillDirectory::create(scratch.join("cache"), false).unwrap();
        let frame = store_light_frame(
            &spill_directory.path,
            "frame",
            invalid,
            None,
            None,
            source_stats,
        )
        .unwrap();

        let error = stack_stored_frames(
            vec![frame],
            Some(spill_directory),
            dimensions,
            ImageMetadata::default(),
            StackConfig::mean(),
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            Error::NonFiniteImageSample {
                index: 0,
                channel: 2,
                pixel: 1,
                value: f32::NEG_INFINITY,
            }
        ));
    }

    fn norm_params_for(cache: &LightCache, normalization: Normalization) -> Option<Vec<FrameNorm>> {
        normalization::compute_light_frame_norms(
            &cache.frames,
            cache.core.dimensions,
            normalization,
            &cache.core.cancel,
        )
        .unwrap()
    }

    fn source_stats(cache: &LightCache) -> impl Iterator<Item = &FrameStats> {
        cache.frames.iter().map(|frame| &frame.source_stats)
    }

    fn make_uniform_frames(pixel_counts: usize, values: &[f32]) -> LightCache {
        let dims = ImageDimensions::new((pixel_counts, 1), 1);
        let images = values
            .iter()
            .map(|&v| LinearImage::from_pixels(dims, vec![v; pixel_counts]))
            .collect();
        make_test_cache(images)
    }

    fn make_rgb_frames(pixels: usize, frame_values: &[[f32; 3]]) -> LightCache {
        let dims = ImageDimensions::new((pixels, 1), 3);
        let images = frame_values
            .iter()
            .map(|rgb| {
                let data: Vec<f32> = (0..pixels).flat_map(|_| rgb.iter().copied()).collect();
                LinearImage::from_pixels(dims, data)
            })
            .collect();
        make_test_cache(images)
    }

    fn assert_norm_identity(params: &[FrameNorm]) {
        for fn_ in params {
            for np in &fn_.channels {
                assert!(
                    (np.gain - 1.0).abs() < 1e-5,
                    "Gain should be ~1.0, got {}",
                    np.gain
                );
                assert!(
                    np.offset.abs() < 1e-5,
                    "Offset should be ~0.0, got {}",
                    np.offset
                );
            }
        }
    }

    fn assert_channel_near(result: &PixelData, channel: usize, expected: f32, tol: f32) {
        for &pixel in result.channel(channel).pixels() {
            assert!(
                (pixel - expected).abs() < tol,
                "Channel {} should be ~{}, got {}",
                channel,
                expected,
                pixel
            );
        }
    }

    fn mean_sample(values: &[f32], weights: &[f32]) -> CombinedSample {
        CombinedSample::from_all(math::sum::mean_f32(values), weights)
    }

    #[test]
    fn test_stack_empty_paths() {
        let paths: Vec<PathBuf> = vec![];
        let result = stack(
            &paths,
            StackConfig::default(),
            ProgressCallback::default(),
            CancelToken::never(),
        );
        assert!(matches!(result.unwrap_err(), Error::NoFrames));
    }

    #[test]
    fn test_stack_images_empty() {
        let result = stack_images(
            Vec::new(),
            StackConfig::default(),
            ProgressCallback::default(),
            CancelToken::never(),
        );
        assert!(matches!(result.unwrap_err(), Error::NoFrames));
    }

    #[test]
    fn test_stack_nonexistent_file() {
        let paths = vec![PathBuf::from("/nonexistent/image.fits")];
        let result = stack(
            &paths,
            StackConfig::default(),
            ProgressCallback::default(),
            CancelToken::never(),
        );
        assert!(matches!(result.unwrap_err(), Error::ImageLoad { .. }));
    }

    #[test]
    fn test_stack_rejects_invalid_config_before_loading() {
        let paths = vec![
            PathBuf::from("/a.fits"),
            PathBuf::from("/b.fits"),
            PathBuf::from("/c.fits"),
        ];
        let error = stack(
            &paths,
            StackConfig::weighted(vec![1.0, 2.0]),
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            Error::Config(StackConfigError::ManualWeightCountMismatch {
                expected: 3,
                actual: 2,
            })
        ));

        let error = stack(
            &paths,
            StackConfig::sigma_clipped(-1.0),
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            Error::Config(StackConfigError::InvalidSigmaLow { value: -1.0 })
        ));
    }

    #[test]
    fn test_stack_images_in_memory_mean() {
        // In-memory stacking must match the documented mean: (10 + 20 + 30)/3 = 20.
        let dims = ImageDimensions::new((4, 4), 1);
        let images = vec![
            LinearImage::from_pixels(dims, vec![10.0; 16]),
            LinearImage::from_pixels(dims, vec![20.0; 16]),
            LinearImage::from_pixels(dims, vec![30.0; 16]),
        ];
        let config = StackConfig {
            method: CombineMethod::Mean(Rejection::None),
            ..Default::default()
        };
        let frames = images.into_iter().map(StackFrame::from).collect();
        let result = stack_images(
            frames,
            config,
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .unwrap()
        .image;
        assert_eq!(result.channels(), 1);
        for &p in result.channel(0).pixels() {
            assert!((p - 20.0).abs() < 1e-4, "expected 20.0, got {p}");
        }
    }

    #[test]
    fn test_stack_images_dimension_errors() {
        let a = LinearImage::from_pixels(ImageDimensions::new((4, 4), 1), vec![1.0; 16]);
        let b = LinearImage::from_pixels(ImageDimensions::new((2, 2), 1), vec![1.0; 4]);
        let result = stack_images(
            vec![a.into(), b.into()],
            StackConfig::default(),
            ProgressCallback::default(),
            CancelToken::never(),
        );
        assert!(matches!(
            result.unwrap_err(),
            Error::DimensionMismatch { index: 1, .. }
        ));

        let frame = stack_frame(
            LinearImage::from_pixels(ImageDimensions::new((4, 4), 1), vec![1.0; 16]),
            Some(Buffer2::new_filled(2, 2, 1.0)),
            None,
        );
        let error = stack_images(
            vec![frame],
            StackConfig::default(),
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            Error::WarpPlaneDimensionMismatch {
                index: 0,
                plane: "coverage",
                expected_width: 4,
                expected_height: 4,
                actual_width: 2,
                actual_height: 2,
            }
        ));

        let frame = stack_frame(
            LinearImage::from_pixels(ImageDimensions::new((4, 4), 1), vec![1.0; 16]),
            None,
            Some(Buffer2::new_filled(2, 2, 1.0)),
        );
        let error = stack_images(
            vec![frame],
            StackConfig::default(),
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            Error::WarpPlaneDimensionMismatch {
                index: 0,
                plane: "confidence",
                expected_width: 4,
                expected_height: 4,
                actual_width: 2,
                actual_height: 2,
            }
        ));
    }

    #[test]
    fn stack_images_rejects_invalid_warp_quality_values() {
        let dims = ImageDimensions::new((2, 1), 1);
        for (coverage, confidence, expected_plane, expected_value) in [
            (
                Some(Buffer2::new(2, 1, vec![1.0, 1.1])),
                None,
                "coverage",
                1.1,
            ),
            (
                None,
                Some(Buffer2::new(2, 1, vec![1.0, -0.1])),
                "confidence",
                -0.1,
            ),
        ] {
            let error = stack_images(
                vec![stack_frame(
                    LinearImage::from_pixels(dims, vec![1.0; 2]),
                    coverage,
                    confidence,
                )],
                StackConfig::default(),
                ProgressCallback::default(),
                CancelToken::never(),
            )
            .unwrap_err();
            assert!(matches!(
                error,
                Error::InvalidWarpPlaneValue {
                    index: 0,
                    plane,
                    pixel: 1,
                    value,
                } if plane == expected_plane && value == expected_value
            ));
        }
    }

    #[test]
    fn stack_images_rejects_each_nonfinite_sample_class_with_location() {
        let dimensions = ImageDimensions::new((2, 2), 3);
        let finite = LinearImage::from_planar_channels(
            dimensions,
            [vec![1.0; 4], vec![1.0; 4], vec![1.0; 4]],
        );

        for invalid_value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let invalid = LinearImage::from_planar_channels(
                dimensions,
                [
                    vec![1.0; 4],
                    vec![2.0, 2.0, invalid_value, 2.0],
                    vec![3.0; 4],
                ],
            );
            let error = stack_images(
                vec![finite.clone().into(), invalid.into()],
                StackConfig::mean(),
                ProgressCallback::default(),
                CancelToken::never(),
            )
            .unwrap_err();

            let Error::NonFiniteImageSample {
                index,
                channel,
                pixel,
                value,
            } = error
            else {
                panic!("expected a non-finite image sample error, got {error:?}");
            };
            assert_eq!(index, 1);
            assert_eq!(channel, 1);
            assert_eq!(pixel, 2);
            assert_eq!(value.to_bits(), invalid_value.to_bits());
        }
    }

    #[test]
    fn cancelled_stack_returns_cancelled_error() {
        let a = LinearImage::from_pixels(ImageDimensions::new((4, 4), 1), vec![1.0; 16]);
        let mut invalid_pixels = vec![2.0; 16];
        invalid_pixels[0] = f32::NAN;
        let b = LinearImage::from_pixels(ImageDimensions::new((4, 4), 1), invalid_pixels);
        let cancel = CancelToken::new();
        cancel.cancel();
        let result = stack_images(
            vec![a.into(), b.into()],
            StackConfig::default(),
            ProgressCallback::default(),
            cancel,
        );
        assert!(matches!(result.unwrap_err(), Error::Cancelled));
    }

    #[test]
    fn coverage_excludes_uncovered_frames() {
        // 2 frames, 2 px. Frame B does not cover pixel 1 (coverage 0) → pixel 1 is A alone.
        let dims = ImageDimensions::new((2, 1), 1);
        let a = LinearImage::from_pixels(dims, vec![10.0, 10.0]);
        let b = LinearImage::from_pixels(dims, vec![20.0, 20.0]);
        let config = StackConfig {
            method: CombineMethod::Mean(Rejection::None),
            normalization: Normalization::None,
            ..Default::default()
        };
        let frames = vec![
            stack_frame(a, Some(Buffer2::new(2, 1, vec![1.0, 1.0])), None),
            stack_frame(b, Some(Buffer2::new(2, 1, vec![1.0, 0.0])), None),
        ];
        let out = stack_images(
            frames,
            config,
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .unwrap()
        .image;
        let px = out.channel(0).pixels();
        assert!(
            (px[0] - 15.0).abs() < 1e-4,
            "px0 = mean(10,20) = 15, got {}",
            px[0]
        );
        assert!(
            (px[1] - 10.0).abs() < 1e-4,
            "px1 = A only = 10, got {}",
            px[1]
        );
    }

    #[test]
    fn common_coverage_makes_reference_norms_and_noise_weights_fill_invariant() {
        let dims = ImageDimensions::new((6, 1), 1);
        let coverage = Buffer2::new(6, 1, vec![0.0, 0.0, 1.0, 1.0, 0.0, 0.0]);
        let make_cache = |fill: [f32; 4]| {
            let frames = [
                [fill[0], fill[1], 10.0, 14.0, fill[2], fill[3]],
                [fill[3], fill[2], 20.0, 28.0, fill[1], fill[0]],
                [fill[1], fill[3], 30.0, 32.0, fill[0], fill[2]],
            ]
            .into_iter()
            .zip([(12.0, 2.0), (24.0, 4.0), (31.0, 1.0)])
            .map(|(pixels, (median, mad))| {
                let mut frame = stack_frame(
                    LinearImage::from_pixels(dims, pixels.to_vec()),
                    Some(coverage.clone()),
                    None,
                );
                frame.source_stats = FrameStats {
                    channels: [ChannelStats { median, mad }].into_iter().collect(),
                    quantization_sigma: None,
                };
                frame
            })
            .collect();
            LightCache::from_stack_frames(
                frames,
                &CacheConfig::default(),
                Normalization::Multiplicative,
                ProgressCallback::default(),
                CancelToken::never(),
            )
            .unwrap()
        };

        let caches = [
            make_cache([0.0, 0.0, 0.0, 0.0]),
            make_cache([-1e20, 1e20, -123_456.0, 789_012.0]),
        ];
        for cache in &caches {
            let norms = cache.frame_norms.as_ref().unwrap();
            assert_eq!(norms[0].channels[0].gain, 31.0 / 12.0);
            assert_eq!(norms[1].channels[0].gain, 31.0 / 24.0);
            assert_eq!(norms[2].channels[0].gain, 1.0);
            assert!(norms.iter().all(|norm| norm.channels[0].offset == 0.0));

            let weights =
                resolve_weights(&Weighting::Noise, source_stats(cache), Some(norms)).unwrap();
            assert!((weights[0] - 36.0 / 1033.0).abs() < 1e-7);
            assert!((weights[1] - 36.0 / 1033.0).abs() < 1e-7);
            assert!((weights[2] - 961.0 / 1033.0).abs() < 1e-7);
        }

        let config = StackConfig {
            method: CombineMethod::Mean(Rejection::None),
            weighting: Weighting::Noise,
            normalization: Normalization::Multiplicative,
            ..Default::default()
        };
        let first = run_stacking_weighted(&caches[0], &config);
        let second = run_stacking_weighted(&caches[1], &config);
        assert_eq!(
            first.image.channel(0).pixels(),
            second.image.channel(0).pixels()
        );
        assert_eq!(first.image.channel(0).pixels()[0..2], [0.0, 0.0]);
        assert!((first.image.channel(0).pixels()[2] - 30_690.0 / 1033.0).abs() < 1e-5);
        assert!((first.image.channel(0).pixels()[3] - 33_356.0 / 1033.0).abs() < 1e-5);
        assert_eq!(first.image.channel(0).pixels()[4..6], [0.0, 0.0]);
    }

    #[test]
    fn only_normalization_requires_common_coverage() {
        let dims = ImageDimensions::new((2, 1), 1);
        let frames = || {
            vec![
                stack_frame(
                    LinearImage::from_pixels(dims, vec![1.0, 2.0]),
                    Some(Buffer2::new(2, 1, vec![1.0, 0.0])),
                    None,
                ),
                stack_frame(
                    LinearImage::from_pixels(dims, vec![3.0, 4.0]),
                    Some(Buffer2::new(2, 1, vec![0.0, 1.0])),
                    None,
                ),
            ]
        };
        let error = stack_images(
            frames(),
            StackConfig {
                normalization: Normalization::Global,
                ..Default::default()
            },
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .unwrap_err();
        assert!(matches!(error, Error::NoCommonCoverage));

        let product = stack_images(
            frames(),
            StackConfig {
                normalization: Normalization::None,
                ..Default::default()
            },
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .unwrap();
        assert_eq!(product.image.channel(0).pixels(), &[1.0, 4.0]);
    }

    #[test]
    fn confidence_weights_contributions_independently_of_coverage() {
        // px0: A (q 1, val 10) + B (q .5, val 20) → 40/3. At px1 B has zero
        // confidence, so only A contributes statistically while both frames retain geometric
        // coverage.
        let dims = ImageDimensions::new((2, 1), 1);
        let a = LinearImage::from_pixels(dims, vec![10.0, 10.0]);
        let b = LinearImage::from_pixels(dims, vec![20.0, 20.0]);
        let config = StackConfig {
            method: CombineMethod::Mean(Rejection::None),
            normalization: Normalization::None,
            ..Default::default()
        };
        let frames = vec![
            stack_frame(
                a,
                Some(Buffer2::new(2, 1, vec![1.0, 1.0])),
                Some(Buffer2::new(2, 1, vec![1.0, 1.0])),
            ),
            stack_frame(
                b,
                Some(Buffer2::new(2, 1, vec![1.0, 1.0])),
                Some(Buffer2::new(2, 1, vec![0.5, 0.0])),
            ),
        ];
        let product = stack_images(
            frames,
            config,
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .unwrap();
        assert!(
            (product.image.channel(0)[0] - 40.0 / 3.0).abs() < 1e-4,
            "expected 40/3, got {}",
            product.image.channel(0)[0]
        );
        assert_eq!(product.image.channel(0)[1], 10.0);
        assert_eq!(product.coverage.pixels(), &[1.0, 1.0]);
        assert_eq!(product.weight.channel(0).pixels(), &[1.5, 1.0]);
        let linear_variance = product.linear_variance.as_ref().unwrap();
        assert!(
            (linear_variance.channel(0)[0] - 5.0 / 9.0).abs() < 1e-6,
            "variance = {}",
            linear_variance.channel(0)[0]
        );
        assert_eq!(linear_variance.channel(0)[1], 1.0);
    }

    #[test]
    fn signed_uniform_warp_and_weighted_combine_preserve_dc() {
        let dims = ImageDimensions::new((24, 20), 1);
        let expected = -0.7;
        let source = LinearImage::from_pixels(dims, vec![expected; dims.pixel_count()]);
        let transform = WarpTransform::new(Transform::translation(glam::DVec2::new(-2.37, 1.43)));
        let config = StackConfig {
            method: CombineMethod::Mean(Rejection::None),
            normalization: Normalization::None,
            ..Default::default()
        };

        for method in [
            InterpolationMethod::Nearest,
            InterpolationMethod::Bilinear,
            InterpolationMethod::Bicubic,
            InterpolationMethod::Lanczos2,
            InterpolationMethod::Lanczos3,
            InterpolationMethod::Lanczos4,
        ] {
            let warped = resample::warp(
                &source,
                &transform,
                &config::test_support::warp_params(method),
            );
            let frames = vec![
                StackFrame::from(source.clone()),
                StackFrame::registered(&source, warped),
            ];
            let product = stack_images(
                frames,
                config.clone(),
                ProgressCallback::default(),
                CancelToken::never(),
            )
            .unwrap();
            for (pixel, &actual) in product.image.channel(0).pixels().iter().enumerate() {
                assert!(
                    (actual - expected).abs() < 2e-5,
                    "{method:?} pixel {pixel}: expected {expected}, got {actual}"
                );
            }
        }
    }

    #[test]
    fn registered_global_normalization_uses_paired_signal_samples() {
        let dims = ImageDimensions::new((64, 48), 1);
        let pixels = (0..dims.height())
            .flat_map(|y| (0..dims.width()).map(move |x| 0.2 + x as f32 * 0.01 + y as f32 * 0.02))
            .collect();
        let source = LinearImage::from_pixels(dims, pixels);
        let params = config::test_support::warp_params(InterpolationMethod::Bilinear);
        let frames = vec![
            StackFrame::registered(
                &source,
                resample::warp(&source, &WarpTransform::new(Transform::identity()), &params),
            ),
            StackFrame::registered(
                &source,
                resample::warp(
                    &source,
                    &WarpTransform::new(Transform::translation(glam::DVec2::splat(0.5))),
                    &params,
                ),
            ),
        ];
        let cache = LightCache::from_stack_frames(
            frames,
            &CacheConfig::default(),
            Normalization::Global,
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .unwrap();
        let norms = cache.frame_norms.as_ref().unwrap();

        assert_eq!(norms[0].channels[0].gain, 1.0);
        assert!(
            (norms[1].channels[0].gain - 1.0).abs() < 1e-3,
            "half-pixel interpolation changed photometric gain to {}",
            norms[1].channels[0].gain
        );
    }

    #[test]
    fn registered_noise_weight_applies_half_pixel_confidence_once() {
        let dims = ImageDimensions::new((64, 48), 1);
        let mut state = 0x1234_5678_u32;
        let pixels = (0..dims.pixel_count())
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                state as f32 / u32::MAX as f32 - 0.5
            })
            .collect();
        let source = LinearImage::from_pixels(dims, pixels);
        let params = config::test_support::warp_params(InterpolationMethod::Bilinear);
        let frames = vec![
            StackFrame::registered(
                &source,
                resample::warp(&source, &WarpTransform::new(Transform::identity()), &params),
            ),
            StackFrame::registered(
                &source,
                resample::warp(
                    &source,
                    &WarpTransform::new(Transform::translation(glam::DVec2::splat(0.5))),
                    &params,
                ),
            ),
        ];
        let cache = LightCache::from_stack_frames(
            frames,
            &CacheConfig::default(),
            Normalization::None,
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .unwrap();
        let base_weights = resolve_weights(&Weighting::Noise, source_stats(&cache), None).unwrap();
        assert_eq!(base_weights, [0.5, 0.5]);

        let pixel = 12 * dims.width() + 12;
        let identity_confidence = cache.frames[0]
            .confidence
            .as_ref()
            .unwrap()
            .chunk(pixel, pixel + 1)[0];
        let half_pixel_confidence = cache.frames[1]
            .confidence
            .as_ref()
            .unwrap()
            .chunk(pixel, pixel + 1)[0];
        let effective_ratio =
            base_weights[1] * half_pixel_confidence / (base_weights[0] * identity_confidence);
        assert!((effective_ratio - 4.0).abs() < 1e-6);
        assert!((effective_ratio - 16.0).abs() > 1.0);

        let product = run_stacking_weighted(
            &cache,
            &StackConfig {
                method: CombineMethod::Mean(Rejection::None),
                weighting: Weighting::Noise,
                normalization: Normalization::None,
                small_n: SmallN::none(),
                ..Default::default()
            },
        );
        assert!((product.weight.channel(0)[pixel] - 2.5).abs() < 1e-6);
    }

    #[test]
    fn coverage_none_frame_counts_fully() {
        // An unwarped reference with no quality planes must have full support and unit confidence.
        let dims = ImageDimensions::new((1, 1), 1);
        let frames = vec![
            StackFrame::from(LinearImage::from_pixels(dims, vec![10.0])),
            stack_frame(
                LinearImage::from_pixels(dims, vec![20.0]),
                Some(Buffer2::new(1, 1, vec![1.0])),
                None,
            ),
        ];
        let config = StackConfig {
            method: CombineMethod::Mean(Rejection::None),
            normalization: Normalization::None,
            ..Default::default()
        };
        let v = stack_images(
            frames,
            config,
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .unwrap()
        .image
        .channel(0)
        .pixels()[0];
        assert!(
            (v - 15.0).abs() < 1e-4,
            "None frame counts fully: mean(10,20) = 15, got {v}"
        );
    }

    #[test]
    fn coverage_zero_in_all_frames_fills_zero() {
        // 1 frame, 2 px; pixel 1 uncovered → no contributor → 0 fill (matches the warp border).
        let dims = ImageDimensions::new((2, 1), 1);
        let a = LinearImage::from_pixels(dims, vec![10.0, 10.0]);
        let config = StackConfig {
            method: CombineMethod::Mean(Rejection::None),
            normalization: Normalization::None,
            ..Default::default()
        };
        let frames = vec![stack_frame(
            a,
            Some(Buffer2::new(2, 1, vec![1.0, 0.0])),
            None,
        )];
        let out = stack_images(
            frames,
            config,
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .unwrap()
        .image;
        let px = out.channel(0).pixels();
        assert!(
            (px[0] - 10.0).abs() < 1e-4,
            "covered px = 10, got {}",
            px[0]
        );
        assert_eq!(px[1], 0.0, "uncovered px must be 0 fill, got {}", px[1]);
    }

    #[test]
    fn coverage_keeps_real_values_from_sigma_rejection_at_sparse_edges() {
        // The artifact this fixes: an edge covered by 2/5 frames. Without coverage the 3 zero
        // border-fills dominate SigmaClip (median 0) and reject the real 0.1 values → dark edge.
        // Coverage excludes the fills → only the 2 real frames combine.
        let dims = ImageDimensions::new((1, 1), 1);
        let frames: Vec<StackFrame> = [0.1, 0.1, 0.0, 0.0, 0.0]
            .iter()
            .zip([1.0, 1.0, 0.0, 0.0, 0.0])
            .map(|(&v, c)| {
                stack_frame(
                    LinearImage::from_pixels(dims, vec![v]),
                    Some(Buffer2::new(1, 1, vec![c])),
                    None,
                )
            })
            .collect();
        let config = StackConfig {
            method: CombineMethod::Mean(Rejection::sigma_clip(2.5)),
            normalization: Normalization::None,
            ..Default::default()
        };
        let v = stack_images(
            frames,
            config,
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .unwrap()
        .image
        .channel(0)
        .pixels()[0];
        assert!(
            (v - 0.1).abs() < 1e-4,
            "edge should be the mean of the 2 covering frames (0.1), got {v}"
        );

        // Cross-check: the same frames with no coverage maps drag the pixel toward 0.
        let frames2: Vec<StackFrame> = [0.1, 0.1, 0.0, 0.0, 0.0]
            .iter()
            .map(|&v| LinearImage::from_pixels(dims, vec![v]).into())
            .collect();
        let cfg2 = StackConfig {
            method: CombineMethod::Mean(Rejection::None),
            normalization: Normalization::None,
            ..Default::default()
        };
        let dark = stack_images(
            frames2,
            cfg2,
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .unwrap()
        .image
        .channel(0)
        .pixels()[0];
        assert!(
            dark < 0.05,
            "without coverage the border-fills pull the edge dark (~0.04), got {dark}"
        );
    }

    #[test]
    fn stack_product_carries_geometry_planes() {
        // End-to-end: stack_images threads the geometry planes onto StackProduct. 2 frames, 2 px;
        // frame B doesn't cover px1. Equal weights, mean combine.
        //   px0: both cover → coverage 2/2 = 1.0, weight 2,   variance (1+1)/2² = 0.5
        //   px1: A only     → coverage 1/2 = 0.5, weight 1,   variance 1/1²     = 1.0
        let dims = ImageDimensions::new((2, 1), 1);
        let frames = vec![
            stack_frame(
                LinearImage::from_pixels(dims, vec![10.0, 10.0]),
                Some(Buffer2::new(2, 1, vec![1.0, 1.0])),
                None,
            ),
            stack_frame(
                LinearImage::from_pixels(dims, vec![20.0, 20.0]),
                Some(Buffer2::new(2, 1, vec![1.0, 0.0])),
                None,
            ),
        ];
        let config = StackConfig {
            method: CombineMethod::Mean(Rejection::None),
            normalization: Normalization::None,
            ..Default::default()
        };
        let r = stack_images(
            frames,
            config,
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .unwrap();
        let linear_variance = r.linear_variance.as_ref().unwrap();
        assert_eq!(r.coverage[0], 1.0);
        assert_eq!(r.weight.channel(0)[0], 2.0);
        assert_eq!(linear_variance.channel(0)[0], 0.5);
        assert_eq!(r.coverage[1], 0.5);
        assert_eq!(r.weight.channel(0)[1], 1.0);
        assert_eq!(linear_variance.channel(0)[1], 1.0);
    }

    #[test]
    fn rejection_emits_channel_shaped_survivor_weight_and_linear_variance() {
        // Percentile clipping removes each channel's high value, hence a different source frame:
        // R keeps f0/f1, G keeps f1/f2, B keeps f0/f2. Manual weights [1,2,3] normalize to
        // [1/6,2/6,3/6].
        let dims = ImageDimensions::new((1, 1), 3);
        let frames = vec![
            LinearImage::from_pixels(dims, vec![1.0, 100.0, 1.0]).into(),
            LinearImage::from_pixels(dims, vec![2.0, 2.0, 100.0]).into(),
            LinearImage::from_pixels(dims, vec![100.0, 3.0, 3.0]).into(),
        ];
        let config = StackConfig {
            method: CombineMethod::Mean(Rejection::Percentile(PercentileClipConfig::new(
                0.0, 34.0,
            ))),
            weighting: Weighting::Manual(vec![1.0, 2.0, 3.0]),
            normalization: Normalization::None,
            small_n: SmallN::none(),
            ..Default::default()
        };

        let result = stack_images(
            frames,
            config,
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .unwrap();

        assert_eq!(result.coverage[0], 1.0);
        let expected_values = [5.0 / 3.0, 13.0 / 5.0, 5.0 / 2.0];
        let expected_weights = [3.0 / 6.0, 5.0 / 6.0, 4.0 / 6.0];
        let expected_linear_variances = [5.0 / 9.0, 13.0 / 25.0, 10.0 / 16.0];
        let linear_variance = result.linear_variance.as_ref().unwrap();
        assert!(matches!(&result.weight, QualityMap::PerChannel(_)));
        assert!(matches!(linear_variance, QualityMap::PerChannel(_)));
        for channel in 0..3 {
            assert!(
                (result.image.channel(channel)[0] - expected_values[channel]).abs() < 1e-6,
                "channel {channel} value"
            );
            assert!(
                (result.weight.channel(channel)[0] - expected_weights[channel]).abs() < 1e-6,
                "channel {channel} weight"
            );
            assert!(
                (linear_variance.channel(channel)[0] - expected_linear_variances[channel]).abs()
                    < 1e-6,
                "channel {channel} variance"
            );
        }
        assert_ne!(result.weight.channel(0)[0], result.weight.channel(1)[0]);
        assert_ne!(linear_variance.channel(1)[0], linear_variance.channel(2)[0]);
    }

    #[test]
    fn median_quality_uses_equal_weights_and_has_no_linear_variance() {
        let dims = ImageDimensions::new((8, 1), 1);
        let mk = |base: f32, spread: f32| -> Vec<f32> {
            (0..8).map(|i| base + i as f32 * spread / 7.0).collect()
        };
        let frames = || -> Vec<StackFrame> {
            vec![
                LinearImage::from_pixels(dims, mk(100.0, 1.0)).into(),
                LinearImage::from_pixels(dims, mk(100.0, 20.0)).into(),
                LinearImage::from_pixels(dims, mk(100.0, 2.0)).into(),
            ]
        };
        let stack = |config| {
            stack_images(
                frames(),
                config,
                ProgressCallback::default(),
                CancelToken::never(),
            )
            .unwrap()
        };

        let explicit = stack(StackConfig {
            method: CombineMethod::Median,
            weighting: Weighting::Noise,
            normalization: Normalization::None,
            ..Default::default()
        });
        assert!(explicit.linear_variance.is_none());
        for p in 0..8 {
            let expected = 100.0 + p as f32 * 2.0 / 7.0;
            assert!((explicit.image.channel(0)[p] - expected).abs() < 1e-5);
            assert_eq!(explicit.coverage[p], 1.0);
            assert_eq!(
                explicit.weight.channel(0)[p],
                3.0,
                "median quality must count unit-confidence contributors (= 3)"
            );
        }

        for (name, config) in [
            ("default", StackConfig::default()),
            ("sigma", StackConfig::sigma_clipped(2.5)),
            ("linear fit", StackConfig::linear_fit(3.0)),
            ("GESD", StackConfig::gesd()),
            ("flat", StackConfig::flat()),
            ("light", StackConfig::light()),
            (
                "manual weighting",
                StackConfig::weighted(vec![1.0, 2.0, 3.0]),
            ),
        ] {
            let downgraded = stack(config);
            assert!(
                downgraded.linear_variance.is_none(),
                "{name} must expose no linear variance after its small-N median downgrade"
            );
            assert_eq!(
                downgraded.weight.channel(0).pixels(),
                &[3.0; 8],
                "{name} median fallback must count unit-confidence contributors"
            );
        }

        let linear_fallback = stack(StackConfig {
            method: CombineMethod::Mean(Rejection::sigma_clip(2.5)),
            small_n: SmallN {
                min_frames: 4,
                fallback: CombineMethod::Mean(Rejection::None),
            },
            ..Default::default()
        });
        assert_eq!(
            linear_fallback.linear_variance.unwrap().channel(0).pixels(),
            &[1.0 / 3.0; 8]
        );
    }

    #[test]
    fn test_norm_identity_for_identical_frames() {
        // Both Global and Multiplicative should produce identity for identical frames
        let cache = make_uniform_frames(16, &[5.0, 5.0, 5.0]);

        for mode in [Normalization::Global, Normalization::Multiplicative] {
            let params = norm_params_for(&cache, mode).unwrap();
            assert_eq!(params.len(), 3);
            assert_norm_identity(&params);
        }
    }

    #[test]
    fn test_global_norm_offset_correction() {
        // Same scale, different median -> gain ~1.0, offset ~-100
        let dims = ImageDimensions::new((4, 4), 1);
        let frame0: Vec<f32> = (0..16).map(|i| 100.0 + i as f32).collect();
        let frame1: Vec<f32> = (0..16).map(|i| 200.0 + i as f32).collect();
        let cache = make_test_cache(vec![
            LinearImage::from_pixels(dims, frame0),
            LinearImage::from_pixels(dims, frame1),
        ]);
        let params = norm_params_for(&cache, Normalization::Global).unwrap();

        assert_norm_identity(&params[..1]);
        assert!(
            (params[1].channels[0].gain - 1.0).abs() < 0.1,
            "Gain should be ~1.0, got {}",
            params[1].channels[0].gain
        );
        assert!(
            (params[1].channels[0].offset - (-100.0)).abs() < 1.0,
            "Offset should be ~-100.0, got {}",
            params[1].channels[0].offset
        );
    }

    #[test]
    fn test_global_norm_scale_correction() {
        // Frame 1 has ~2x the spread -> gain ~0.5
        let dims = ImageDimensions::new((10, 10), 1);
        let frame0: Vec<f32> = (0..100).map(|i| 90.0 + (i as f32) * 20.0 / 99.0).collect();
        let frame1: Vec<f32> = (0..100).map(|i| 80.0 + (i as f32) * 40.0 / 99.0).collect();
        let cache = make_test_cache(vec![
            LinearImage::from_pixels(dims, frame0),
            LinearImage::from_pixels(dims, frame1),
        ]);
        let params = norm_params_for(&cache, Normalization::Global).unwrap();

        assert!(
            (params[1].channels[0].gain - 0.5).abs() < 0.15,
            "Gain should be ~0.5, got {}",
            params[1].channels[0].gain
        );
    }

    #[test]
    fn test_global_norm_stacking_corrects_offset() {
        // After normalization, both frames should be brought to reference level
        let cache = make_uniform_frames(16, &[100.0, 150.0]);
        let norm_params = norm_params_for(&cache, Normalization::Global).unwrap();

        let result = cache.process_chunked_weighted(None, Some(&norm_params), |values, w, _| {
            mean_sample(values, w)
        });
        assert_channel_near(&result.pixels, 0, 100.0, 1.0);
    }

    #[test]
    fn test_multiplicative_norm_scales_by_median_ratio() {
        let cache = make_uniform_frames(16, &[100.0, 200.0]);
        let params = norm_params_for(&cache, Normalization::Multiplicative).unwrap();

        assert_norm_identity(&params[..1]);
        assert!(
            (params[1].channels[0].gain - 0.5).abs() < 1e-5,
            "Gain should be 0.5, got {}",
            params[1].channels[0].gain
        );
        assert!(
            params[1].channels[0].offset.abs() < 1e-5,
            "Offset should be 0.0, got {}",
            params[1].channels[0].offset
        );
    }

    #[test]
    fn test_multiplicative_norm_no_offset() {
        // Multiplicative should only scale, never shift
        let dims = ImageDimensions::new((10, 10), 1);
        let frame0: Vec<f32> = (0..100).map(|i| 90.0 + (i as f32) * 0.2).collect();
        let frame1: Vec<f32> = (0..100).map(|i| 180.0 + (i as f32) * 0.4).collect();
        let cache = make_test_cache(vec![
            LinearImage::from_pixels(dims, frame0),
            LinearImage::from_pixels(dims, frame1),
        ]);
        let params = norm_params_for(&cache, Normalization::Multiplicative).unwrap();

        for fn_ in &params {
            for np in &fn_.channels {
                assert!(
                    np.offset.abs() < f32::EPSILON,
                    "Multiplicative offset must be 0, got {}",
                    np.offset
                );
            }
        }
    }

    #[test]
    fn test_multiplicative_stacking_normalizes_flat_levels() {
        let cache = make_uniform_frames(16, &[100.0, 200.0]);
        let norm_params = norm_params_for(&cache, Normalization::Multiplicative).unwrap();

        let result = cache.process_chunked_weighted(None, Some(&norm_params), |values, w, _| {
            mean_sample(values, w)
        });
        assert_channel_near(&result.pixels, 0, 100.0, 1.0);
    }

    #[test]
    fn test_normalized_stacking_rgb() {
        // Both modes should normalize RGB frames to frame 0's reference levels
        let ref_rgb = [100.0, 200.0, 300.0];

        for (mode, frame1_rgb) in [
            // Global: different offsets per channel
            (Normalization::Global, [120.0, 180.0, 350.0]),
            // Multiplicative: different scale per channel
            (Normalization::Multiplicative, [150.0, 100.0, 600.0]),
        ] {
            let cache = make_rgb_frames(16, &[ref_rgb, frame1_rgb]);
            let norm_params = norm_params_for(&cache, mode).unwrap();

            let result =
                cache.process_chunked_weighted(None, Some(&norm_params), |values, w, _| {
                    mean_sample(values, w)
                });

            for (ch, &expected) in ref_rgb.iter().enumerate() {
                assert_channel_near(&result.pixels, ch, expected, 2.0);
            }
        }
    }

    #[test]
    fn test_dispatch_normalized_vs_unnormalized() {
        let cache = make_uniform_frames(16, &[100.0, 200.0]);
        let norm_params = norm_params_for(&cache, Normalization::Global).unwrap();

        let result_norm =
            cache.process_chunked_weighted(None, Some(&norm_params), |values, w, scratch| {
                Rejection::None.combine_mean_with_quality(values, w, scratch)
            });
        let result_unnorm = cache.process_chunked_weighted(None, None, |values, w, scratch| {
            Rejection::None.combine_mean_with_quality(values, w, scratch)
        });

        let norm_pixel = result_norm.pixels.channel(0)[0];
        let unnorm_pixel = result_unnorm.pixels.channel(0)[0];
        assert!(
            (norm_pixel - 100.0).abs() < 1.0,
            "Normalized should be ~100, got {}",
            norm_pixel
        );
        assert!(
            (unnorm_pixel - 150.0).abs() < 1.0,
            "Unnormalized should be ~150, got {}",
            unnorm_pixel
        );
    }

    #[test]
    fn disk_backed_stack_combines_via_mmap() {
        // Force the disk tier (1-byte memory budget) so the full chunked combine reads
        // memory-mapped `Plane`s: mean(10, 20, 30) = 20 at every pixel.
        let temp_dir = ScratchDirectory::new("lumos_disk_stack_combine_test");

        let dims = ImageDimensions::new((4, 4), 1);
        let mut paths = Vec::new();
        for (i, &v) in [10.0f32, 20.0, 30.0].iter().enumerate() {
            let image = LinearImage::from_pixels(dims, vec![v; 16]);
            let path = temp_dir.join(format!("frame{i}.tiff"));
            image.save(&path).unwrap();
            paths.push(path);
        }

        let config = StackConfig {
            method: CombineMethod::Mean(Rejection::None),
            normalization: Normalization::None,
            cache: CacheConfig {
                available_memory: Some(1), // forces disk-backed (mmap) storage
                ..CacheConfig::with_cache_dir(temp_dir.join("cache"))
            },
            ..Default::default()
        };
        let result = stack(
            &paths,
            config,
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .unwrap()
        .image;
        for &p in result.channel(0).pixels() {
            assert!(
                (p - 20.0).abs() < 1e-4,
                "disk-backed mean should be 20, got {p}"
            );
        }
    }

    #[test]
    fn test_norm_uses_lowest_noise_reference() {
        // Frame 0: median=100, high spread (noisy)
        // Frame 1: median=200, low spread (clean) ← should be reference
        // Frame 2: median=150, medium spread
        //
        // With Global normalization, the reference frame gets IDENTITY params.
        // Other frames are normalized to match the reference.
        let dims = ImageDimensions::new((10, 10), 1);
        // Frame 0: values spread from 80..120 (median=100, MAD=10)
        let f0: Vec<f32> = (0..100).map(|i| 80.0 + (i as f32) * 40.0 / 99.0).collect();
        // Frame 1: values spread from 198..202 (median=200, MAD=1)
        let f1: Vec<f32> = (0..100).map(|i| 198.0 + (i as f32) * 4.0 / 99.0).collect();
        // Frame 2: values spread from 140..160 (median=150, MAD=5)
        let f2: Vec<f32> = (0..100).map(|i| 140.0 + (i as f32) * 20.0 / 99.0).collect();

        let cache = make_test_cache(vec![
            LinearImage::from_pixels(dims, f0),
            LinearImage::from_pixels(dims, f1),
            LinearImage::from_pixels(dims, f2),
        ]);

        let params = norm_params_for(&cache, Normalization::Global).unwrap();

        // Frame 1 (lowest noise) should have identity params
        assert!(
            (params[1].channels[0].gain - 1.0).abs() < 1e-5,
            "Reference frame (1) should have gain=1.0, got {}",
            params[1].channels[0].gain
        );
        assert!(
            params[1].channels[0].offset.abs() < 1e-5,
            "Reference frame (1) should have offset=0.0, got {}",
            params[1].channels[0].offset
        );

        // Frame 0 should NOT have identity (it's not the reference)
        assert!(
            (params[0].channels[0].gain - 1.0).abs() > 0.01
                || params[0].channels[0].offset.abs() > 0.1,
            "Frame 0 should not have identity params: gain={}, offset={}",
            params[0].channels[0].gain,
            params[0].channels[0].offset
        );

        // Frame 2 should NOT have identity either
        assert!(
            (params[2].channels[0].gain - 1.0).abs() > 0.01
                || params[2].channels[0].offset.abs() > 0.1,
            "Frame 2 should not have identity params: gain={}, offset={}",
            params[2].channels[0].gain,
            params[2].channels[0].offset
        );
    }

    #[test]
    fn test_norm_result_matches_lowest_noise_frame() {
        // After global normalization + mean stacking, the result should be
        // at the reference frame's level (the lowest-noise frame).
        let dims = ImageDimensions::new((16, 1), 1);
        // Frame 0: high noise, median ~100
        let f0: Vec<f32> = (0..16).map(|i| 80.0 + (i as f32) * 40.0 / 15.0).collect();
        // Frame 1: low noise, median ~200 ← reference
        let f1: Vec<f32> = (0..16).map(|i| 199.0 + (i as f32) * 2.0 / 15.0).collect();

        let cache = make_test_cache(vec![
            LinearImage::from_pixels(dims, f0),
            LinearImage::from_pixels(dims, f1),
        ]);
        let norm_params = norm_params_for(&cache, Normalization::Global).unwrap();

        // Frame 1 is reference (lower noise), so stacked result should be ~200
        let result = cache.process_chunked_weighted(None, Some(&norm_params), |values, w, _| {
            mean_sample(values, w)
        });
        assert_channel_near(&result.pixels, 0, 200.0, 2.0);
    }

    #[test]
    fn test_noise_weighting_downweights_noisy_frame() {
        // Frame 0: clean, values ~100 (spread 0.5)
        // Frame 1: noisy, values ~200 (spread 20.0)
        // With equal weight: mean ≈ 150
        // With noise weight: w0 = 1/sigma0^2 >> w1 = 1/sigma1^2, so result ≈ 100
        let dims = ImageDimensions::new((100, 1), 1);
        let f0: Vec<f32> = (0..100).map(|i| 99.75 + (i as f32) * 0.5 / 99.0).collect();
        let f1: Vec<f32> = (0..100).map(|i| 190.0 + (i as f32) * 20.0 / 99.0).collect();
        let cache = make_test_cache(vec![
            LinearImage::from_pixels(dims, f0),
            LinearImage::from_pixels(dims, f1),
        ]);

        // sigma0 ≈ MAD*1.4826 (small), sigma1 ≈ MAD*1.4826 (large)
        let sigma0 = math::statistics::mad_to_sigma(cache.frames[0].source_stats.channels[0].mad);
        let sigma1 = math::statistics::mad_to_sigma(cache.frames[1].source_stats.channels[0].mad);
        assert!(
            sigma1 > sigma0 * 5.0,
            "Frame 1 should be much noisier: sigma0={sigma0}, sigma1={sigma1}"
        );

        let weights = resolve_weights(&Weighting::Noise, source_stats(&cache), None).unwrap();
        // Frame 0 should get much higher weight
        assert!(
            weights[0] > weights[1] * 10.0,
            "Clean frame weight ({}) should be >> noisy frame weight ({})",
            weights[0],
            weights[1]
        );
        // Weights sum to 1.0
        assert!((weights.iter().sum::<f32>() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_noise_weighting_equal_noise_gives_equal_weights() {
        // 3 identical frames → equal noise → equal weights
        let cache = make_uniform_frames(100, &[50.0, 50.0, 50.0]);
        // All MADs are 0 for uniform frames → all weights are 0 → returns None
        let weights = resolve_weights(&Weighting::Noise, source_stats(&cache), None);
        assert!(
            weights.is_none(),
            "Uniform frames have zero MAD → no weights (equal weighting fallback)"
        );
    }

    #[test]
    fn test_noise_weighting_with_spread_equal_noise() {
        // 3 frames with identical spread → equal weights
        let dims = ImageDimensions::new((100, 1), 1);
        let make_frame =
            |base: f32| -> Vec<f32> { (0..100).map(|i| base + (i as f32) * 10.0 / 99.0).collect() };
        let cache = make_test_cache(vec![
            LinearImage::from_pixels(dims, make_frame(100.0)),
            LinearImage::from_pixels(dims, make_frame(200.0)),
            LinearImage::from_pixels(dims, make_frame(300.0)),
        ]);
        let weights = resolve_weights(&Weighting::Noise, source_stats(&cache), None).unwrap();
        // All should be ≈ 1/3
        for (i, &w) in weights.iter().enumerate() {
            assert!(
                (w - 1.0 / 3.0).abs() < 1e-4,
                "Frame {i} weight should be ~0.333, got {w}"
            );
        }
    }

    #[test]
    fn test_noise_weighting_with_rejection() {
        // Verify noise weights survive through rejection pipeline.
        // Frame 0: clean (narrow spread ~100)
        // Frame 1: noisy (wide spread ~100)
        // Frame 2: has outlier pixel but clean otherwise (~100)
        // Sigma-clip should reject the outlier; noise weights should favor frame 0.
        let dims = ImageDimensions::new((16, 1), 1);
        let f0: Vec<f32> = (0..16).map(|i| 99.9 + (i as f32) * 0.2 / 15.0).collect();
        let f1: Vec<f32> = (0..16).map(|i| 90.0 + (i as f32) * 20.0 / 15.0).collect();
        let mut f2: Vec<f32> = (0..16).map(|i| 99.8 + (i as f32) * 0.4 / 15.0).collect();
        f2[0] = 999.0; // outlier

        let cache = make_test_cache(vec![
            LinearImage::from_pixels(dims, f0),
            LinearImage::from_pixels(dims, f1),
            LinearImage::from_pixels(dims, f2),
        ]);

        let config = StackConfig {
            method: CombineMethod::Mean(Rejection::sigma_clip(2.0)),
            weighting: Weighting::Noise,
            ..Default::default()
        };
        let result = run_stacking_weighted(&cache, &config);
        let pixel = result.image.channel(0).pixels()[0];
        // Result should be near 100 (clean frame value), not near 999 (outlier)
        assert!(
            (pixel - 100.0).abs() < 10.0,
            "Noise-weighted + sigma-clip should be ~100, got {pixel}"
        );
    }

    #[test]
    fn test_noise_weighting_folds_normalization_gain() {
        // Two frames with identical MAD (σ_A = σ_B). Frame B's normalization gain is 2, so its
        // combined noise is 2σ: w_A ∝ 1/σ², w_B ∝ 1/(2σ)² = w_A/4 → normalized 0.8 / 0.2.
        // Without the pscale² term both weights would come out 0.5.
        let frame_stats = |mad: f32| {
            let mut channels = ArrayVec::new();
            channels.push(ChannelStats { median: 0.5, mad });
            FrameStats {
                channels,
                quantization_sigma: None,
            }
        };
        let frame_norm = |gain: f32| {
            let mut channels = ArrayVec::new();
            channels.push(ChannelNorm { gain, offset: 0.0 });
            FrameNorm { channels }
        };
        let stats = vec![frame_stats(0.01), frame_stats(0.01)];
        let norms = vec![frame_norm(1.0), frame_norm(2.0)];

        let weights = resolve_weights(&Weighting::Noise, &stats, Some(&norms)).unwrap();
        assert!(
            (weights[0] - 0.8).abs() < 1e-6,
            "reference frame weight should be 0.8, got {}",
            weights[0]
        );
        assert!(
            (weights[1] - 0.2).abs() < 1e-6,
            "gain-2 frame weight should be 0.2, got {}",
            weights[1]
        );

        // Identity norms reproduce the unscaled weighting: equal σ → equal weights.
        let identity = vec![frame_norm(1.0), frame_norm(1.0)];
        let equal = resolve_weights(&Weighting::Noise, &stats, Some(&identity)).unwrap();
        assert!((equal[0] - 0.5).abs() < 1e-6 && (equal[1] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn test_manual_weighting_is_scale_invariant() {
        let weights = resolve_weights(&Weighting::Manual(vec![1.0, 2.0, 3.0]), &[], None).unwrap();
        assert_eq!(weights, [1.0_f32 / 6.0, 2.0 / 6.0, 3.0 / 6.0]);

        let scale = f32::MIN_POSITIVE;
        let tiny = resolve_weights(
            &Weighting::Manual(vec![scale, 2.0 * scale, 3.0 * scale]),
            &[],
            None,
        )
        .unwrap();
        assert_eq!(tiny, weights);
    }

    #[test]
    fn test_equal_weighting_returns_none() {
        let weights = resolve_weights(&Weighting::Equal, &[], None);
        assert!(weights.is_none());
    }

    #[test]
    fn test_light_preset_uses_noise_weighting() {
        let config = StackConfig::light();
        assert_eq!(config.weighting, Weighting::Noise);
    }
}
