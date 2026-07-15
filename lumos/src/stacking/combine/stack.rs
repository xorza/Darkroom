//! Unified stacking entry point.
//!
//! Provides `stack()` (from paths) and `stack_images()` (in-memory) as the main API
//! for image stacking operations; both take a [`ProgressCallback`].

use std::path::Path;

use arrayvec::ArrayVec;

use crate::AstroImage;
use crate::io::astro_image::cfa::CfaImage;
use crate::io::astro_image::{AstroImageMetadata, ImageDimensions};
use common::CancelToken;
use imaginarium::Buffer2;

use crate::math;
use crate::math::statistics::ChannelStats;
use crate::stacking::combine::cache::{CfaCache, LightCache};
use crate::stacking::combine::config::{CombineMethod, Normalization, StackConfig, Weighting};
use crate::stacking::combine::error::{Error, StackConfigError};
use crate::stacking::combine::progress::ProgressCallback;
use crate::stacking::frame_store::{FrameStats, SpillDirectory, StackableImage, StoredLightFrame};
use crate::stacking::product::StackProduct;

/// Per-frame, per-channel affine normalization parameters.
///
/// Applied as `normalized = raw * gain + offset`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ChannelNorm {
    pub gain: f32,
    pub offset: f32,
}

impl ChannelNorm {
    pub const IDENTITY: ChannelNorm = ChannelNorm {
        gain: 1.0,
        offset: 0.0,
    };
}

/// Per-frame normalization: one `ChannelNorm` per channel.
#[derive(Debug, Clone)]
pub(crate) struct FrameNorm {
    pub channels: ArrayVec<ChannelNorm, 3>,
}

/// One input frame for [`stack_images`]: the image plus optional per-pixel coverage.
///
/// `coverage` (e.g. produced by warping) marks how much of each output pixel landed on real source
/// data; the combine includes a frame at a pixel only where its coverage is meaningful, weighted by
/// it, so warped-frame borders don't drag the stacked edges dark. `None` means the frame fully
/// covers every pixel (weight 1) — e.g. an unwarped reference. Plain `AstroImage`s convert with
/// `.into()`.
#[derive(Debug)]
pub struct StackFrame {
    pub image: AstroImage,
    pub coverage: Option<Buffer2<f32>>,
}

impl From<AstroImage> for StackFrame {
    fn from(image: AstroImage) -> Self {
        Self {
            image,
            coverage: None,
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
/// A [`StackProduct`] whose coverage is the fraction of frames contributing at each pixel. Its
/// weight and variance planes are computed before per-channel outlier rejection.
///
/// # Errors
///
/// Returns an error if:
/// - No paths are provided
/// - The configuration is invalid or its manual-weight count doesn't match the paths
/// - Image loading fails
/// - Image dimensions don't match
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
    let cache = LightCache::from_paths(paths, &config.cache, progress, cancel)?;
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
/// Each [`StackFrame`] may carry per-pixel `coverage`; the combine then includes a frame at a
/// pixel only where it actually covers, so warped-frame borders don't drag the stacked edges
/// dark. Frames with no coverage count fully everywhere. Plain `AstroImage`s convert via `.into()`.
///
/// Returns an error when the configuration is invalid, manual-weight count doesn't match the frame
/// count, or any image/coverage dimensions differ.
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
        coverage_weighted = frames.iter().any(|f| f.coverage.is_some()),
        "Starting unified stack (in memory)"
    );

    let mut cache = LightCache::from_stack_frames(frames, &config.cache, progress)?;
    cache.core.cancel = cancel;
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
    metadata: AstroImageMetadata,
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
        spill_directory,
        dimensions,
        metadata,
        &config.cache,
        progress,
        cancel,
    );
    let result = run_stacking_weighted(&cache, &config);
    if cache.core.cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    Ok(result)
}

/// Select the best reference frame by lowest average noise (MAD) across channels.
///
/// For each frame, computes the mean MAD across all channels. The frame with the
/// lowest mean MAD is selected — it has the most stable background and will produce
/// the best normalization for all other frames.
fn select_reference_frame(stats: &[FrameStats]) -> usize {
    assert!(!stats.is_empty());
    let mut best_frame = 0;
    let mut best_mad = f32::MAX;

    for (frame_idx, fs) in stats.iter().enumerate() {
        let mad_sum: f32 = fs.channels.iter().map(|c| c.mad).sum();
        let avg_mad = mad_sum / fs.channels.len() as f32;
        if avg_mad < best_mad {
            best_mad = avg_mad;
            best_frame = frame_idx;
        }
    }

    best_frame
}

/// Compute per-frame normalization parameters from pre-computed channel stats.
///
/// Auto-selects the lowest-noise frame as reference. Returns `None` for
/// `Normalization::None`.
///
/// - **Global**: `gain = ref_mad / frame_mad`, `offset = ref_median - frame_median * gain`
/// - **Multiplicative**: `gain = ref_median / frame_median`, `offset = 0`
fn compute_frame_norms(
    stats: &[FrameStats],
    normalization: Normalization,
) -> Option<Vec<FrameNorm>> {
    if normalization == Normalization::None {
        return None;
    }
    let ref_frame = select_reference_frame(stats);
    let channels = stats[0].channels.len();

    let mut params: Vec<FrameNorm> = stats
        .iter()
        .map(|fs| {
            let mut channels = ArrayVec::new();
            channels.extend(std::iter::repeat_n(
                ChannelNorm::IDENTITY,
                fs.channels.len(),
            ));
            FrameNorm { channels }
        })
        .collect();

    for channel in 0..channels {
        let ChannelStats {
            median: ref_median,
            mad: ref_mad,
        } = stats[ref_frame].channels[channel];

        for (frame_idx, fs) in stats.iter().enumerate() {
            if frame_idx == ref_frame {
                continue; // Reference frame keeps IDENTITY
            }

            let ChannelStats {
                median: frame_median,
                mad: frame_mad,
            } = fs.channels[channel];

            let np = match normalization {
                Normalization::Global => {
                    let gain = if frame_mad > f32::EPSILON {
                        ref_mad / frame_mad
                    } else {
                        1.0
                    };
                    ChannelNorm {
                        gain,
                        offset: ref_median - frame_median * gain,
                    }
                }
                Normalization::Multiplicative => {
                    let gain = if frame_median > f32::EPSILON {
                        ref_median / frame_median
                    } else {
                        1.0
                    };
                    ChannelNorm { gain, offset: 0.0 }
                }
                Normalization::None => unreachable!(),
            };

            params[frame_idx].channels[channel] = np;
        }
    }

    tracing::info!(
        frame_count = stats.len(),
        channels,
        ref_frame,
        ?normalization,
        "Computed normalization"
    );

    Some(params)
}

/// Resolve weights from the weighting strategy and pre-computed channel stats.
///
/// Returns normalized weights (sum to 1.0) or `None` for equal weighting.
fn resolve_weights(
    weighting: &Weighting,
    stats: &[FrameStats],
    frame_norms: Option<&[FrameNorm]>,
) -> Option<Vec<f32>> {
    match weighting {
        Weighting::Equal => None,
        Weighting::Noise => {
            assert!(
                !stats.is_empty(),
                "channel stats required for noise weighting"
            );
            // Inverse variance of the frame *as combined*: normalization multiplies the frame by
            // `gain`, scaling its noise to `gain·σ`, so w = 1/(gain·σ)² — the "pscale²" term.
            // Without it a frame scaled up to match the reference is over-weighted by gain².
            let weights: Vec<f32> = stats
                .iter()
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

pub(crate) fn run_stacking(cache: &CfaCache, config: &StackConfig) -> CfaImage {
    let stats = &cache.core.channel_stats;
    let method = config.small_n.resolve(config.method, stats.len());
    warn_if_weights_ignored(method, &config.weighting);
    let frame_norms = compute_frame_norms(stats, config.normalization);
    let weights = resolve_weights(&config.weighting, stats, frame_norms.as_deref());
    let norms = frame_norms.as_deref();

    let pixels = match method {
        CombineMethod::Median => cache.process_chunked(None, norms, |values, _, _| {
            math::statistics::median_f32_mut(values)
        }),
        CombineMethod::Mean(rejection) => {
            cache.process_chunked(weights.as_deref(), norms, move |values, w, scratch| {
                rejection.combine_mean(values, w, scratch)
            })
        }
    };
    CfaImage::from_stacked(pixels, cache.core.metadata.clone(), cache.core.dimensions)
}

/// Coverage-weighted counterpart to [`run_stacking`] for a [`LightCache`]: identical combine math,
/// but each frame contributes only where it covers (`process_chunked_weighted`).
pub(crate) fn run_stacking_weighted(cache: &LightCache, config: &StackConfig) -> StackProduct {
    let stats = &cache.core.channel_stats;
    let method = config.small_n.resolve(config.method, stats.len());
    warn_if_weights_ignored(method, &config.weighting);
    let frame_norms = compute_frame_norms(stats, config.normalization);
    let weights = resolve_weights(&config.weighting, stats, frame_norms.as_deref());
    let norms = frame_norms.as_deref();

    // Median ignores per-frame weights (each contributing frame counts equally), so the geometry
    // planes must too — otherwise the weight/variance maps wouldn't describe the image produced.
    let weighted_combine = matches!(method, CombineMethod::Mean(_));

    let pixels = match method {
        CombineMethod::Median => cache.process_chunked_weighted(None, norms, |values, _, _| {
            math::statistics::median_f32_mut(values)
        }),
        CombineMethod::Mean(rejection) => {
            cache.process_chunked_weighted(weights.as_deref(), norms, move |values, w, scratch| {
                rejection.combine_mean(values, w, scratch)
            })
        }
    };

    let geometry_weights = if weighted_combine {
        weights.as_deref()
    } else {
        None
    };
    let image =
        AstroImage::from_stacked(pixels, cache.core.metadata.clone(), cache.core.dimensions);
    cache.finish_product(image, geometry_weights)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::astro_image::PixelData;
    use crate::stacking::combine::cache_config::CacheConfig;
    use crate::stacking::combine::rejection::Rejection;
    use crate::stacking::frame_store::store_light_frame;
    use crate::{
        io::astro_image::{AstroImage, ImageDimensions},
        stacking::combine::cache::tests::make_test_cache,
    };
    use std::path::PathBuf;

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
            let image = AstroImage::from_planar_channels(dims, [px]);
            // Every other frame gets a partial coverage map (warped-border emulation).
            let coverage = f.is_multiple_of(2).then(|| {
                let mut c = vec![1.0f32; w * h];
                c[0] = 0.3;
                Buffer2::new(w, h, c)
            });
            StackFrame { image, coverage }
        };
        let frames: Vec<StackFrame> = (0..n).map(make_frame).collect();
        let config = StackConfig::light();

        let ram = stack_images(
            frames
                .iter()
                .map(|f| StackFrame {
                    image: f.image.clone(),
                    coverage: f.coverage.clone(),
                })
                .collect(),
            config.clone(),
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .unwrap();

        let cache_dir =
            std::env::temp_dir().join(format!("lumos_tier_test_{}", std::process::id()));
        let spill_directory = SpillDirectory::create(cache_dir, false).unwrap();
        let metadata = frames[0].image.metadata().clone();
        let stored = frames
            .into_iter()
            .enumerate()
            .map(|(i, f)| {
                store_light_frame(&spill_directory.path, &format!("f{i}"), f.image, f.coverage)
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
        assert_eq!(bits(&ram.weight), bits(&disk.weight), "weight differs");
        assert_eq!(
            bits(&ram.variance),
            bits(&disk.variance),
            "variance differs"
        );
    }

    // ========== Helpers ==========

    /// Convenience wrapper: compute stats + norm params from cache (for tests).
    fn norm_params_for(cache: &LightCache, normalization: Normalization) -> Option<Vec<FrameNorm>> {
        compute_frame_norms(&cache.core.channel_stats, normalization)
    }

    fn make_uniform_frames(pixel_counts: usize, values: &[f32]) -> LightCache {
        let dims = ImageDimensions::new((pixel_counts, 1), 1);
        let images = values
            .iter()
            .map(|&v| AstroImage::from_pixels(dims, vec![v; pixel_counts]))
            .collect();
        make_test_cache(images)
    }

    fn make_rgb_frames(pixels: usize, frame_values: &[[f32; 3]]) -> LightCache {
        let dims = ImageDimensions::new((pixels, 1), 3);
        let images = frame_values
            .iter()
            .map(|rgb| {
                let data: Vec<f32> = (0..pixels).flat_map(|_| rgb.iter().copied()).collect();
                AstroImage::from_pixels(dims, data)
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

    // ========== Error Handling ==========

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
            AstroImage::from_pixels(dims, vec![10.0; 16]),
            AstroImage::from_pixels(dims, vec![20.0; 16]),
            AstroImage::from_pixels(dims, vec![30.0; 16]),
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
        let a = AstroImage::from_pixels(ImageDimensions::new((4, 4), 1), vec![1.0; 16]);
        let b = AstroImage::from_pixels(ImageDimensions::new((2, 2), 1), vec![1.0; 4]);
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

        let frame = StackFrame {
            image: AstroImage::from_pixels(ImageDimensions::new((4, 4), 1), vec![1.0; 16]),
            coverage: Some(Buffer2::new(2, 2, vec![1.0; 4])),
        };
        let error = stack_images(
            vec![frame],
            StackConfig::default(),
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            Error::CoverageDimensionMismatch {
                index: 0,
                expected_width: 4,
                expected_height: 4,
                actual_width: 2,
                actual_height: 2,
            }
        ));
    }

    #[test]
    fn cancelled_stack_returns_cancelled_error() {
        let a = AstroImage::from_pixels(ImageDimensions::new((4, 4), 1), vec![1.0; 16]);
        let b = AstroImage::from_pixels(ImageDimensions::new((4, 4), 1), vec![2.0; 16]);
        // A pre-tripped token: the combine bails (one chunk here) and the entry
        // reports `Cancelled` rather than a (discarded) partial result.
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

    // ========== Per-pixel coverage weighting (warp → stack) ==========

    #[test]
    fn coverage_excludes_uncovered_frames() {
        // 2 frames, 2 px. Frame B does not cover pixel 1 (coverage 0) → pixel 1 is A alone.
        let dims = ImageDimensions::new((2, 1), 1);
        let a = AstroImage::from_pixels(dims, vec![10.0, 10.0]);
        let b = AstroImage::from_pixels(dims, vec![20.0, 20.0]);
        let config = StackConfig {
            method: CombineMethod::Mean(Rejection::None),
            normalization: Normalization::None,
            ..Default::default()
        };
        let frames = vec![
            StackFrame {
                image: a,
                coverage: Some(Buffer2::new(2, 1, vec![1.0, 1.0])),
            },
            StackFrame {
                image: b,
                coverage: Some(Buffer2::new(2, 1, vec![1.0, 0.0])),
            },
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
    fn coverage_weights_partial_contributions() {
        // 1 px: A (cov 1.0, val 10) + B (cov 0.5, val 20) → (10·1 + 20·0.5)/1.5 = 40/3.
        let dims = ImageDimensions::new((1, 1), 1);
        let a = AstroImage::from_pixels(dims, vec![10.0]);
        let b = AstroImage::from_pixels(dims, vec![20.0]);
        let config = StackConfig {
            method: CombineMethod::Mean(Rejection::None),
            normalization: Normalization::None,
            ..Default::default()
        };
        let frames = vec![
            StackFrame {
                image: a,
                coverage: Some(Buffer2::new(1, 1, vec![1.0])),
            },
            StackFrame {
                image: b,
                coverage: Some(Buffer2::new(1, 1, vec![0.5])),
            },
        ];
        let out = stack_images(
            frames,
            config,
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .unwrap()
        .image;
        let v = out.channel(0).pixels()[0];
        assert!((v - 40.0 / 3.0).abs() < 1e-4, "expected 40/3, got {v}");
    }

    #[test]
    fn coverage_none_frame_counts_fully() {
        // A `coverage: None` frame (e.g. the unwarped reference) must count fully (weight 1)
        // alongside coverage-carrying frames — not be excluded.
        let dims = ImageDimensions::new((1, 1), 1);
        let frames = vec![
            StackFrame::from(AstroImage::from_pixels(dims, vec![10.0])),
            StackFrame {
                image: AstroImage::from_pixels(dims, vec![20.0]),
                coverage: Some(Buffer2::new(1, 1, vec![1.0])),
            },
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
        let a = AstroImage::from_pixels(dims, vec![10.0, 10.0]);
        let config = StackConfig {
            method: CombineMethod::Mean(Rejection::None),
            normalization: Normalization::None,
            ..Default::default()
        };
        let frames = vec![StackFrame {
            image: a,
            coverage: Some(Buffer2::new(2, 1, vec![1.0, 0.0])),
        }];
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
            .map(|(&v, c)| StackFrame {
                image: AstroImage::from_pixels(dims, vec![v]),
                coverage: Some(Buffer2::new(1, 1, vec![c])),
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
            .map(|&v| AstroImage::from_pixels(dims, vec![v]).into())
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
            StackFrame {
                image: AstroImage::from_pixels(dims, vec![10.0, 10.0]),
                coverage: Some(Buffer2::new(2, 1, vec![1.0, 1.0])),
            },
            StackFrame {
                image: AstroImage::from_pixels(dims, vec![20.0, 20.0]),
                coverage: Some(Buffer2::new(2, 1, vec![1.0, 0.0])),
            },
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
        assert_eq!(r.coverage[0], 1.0);
        assert_eq!(r.weight[0], 2.0);
        assert_eq!(r.variance[0], 0.5);
        assert_eq!(r.coverage[1], 0.5);
        assert_eq!(r.weight[1], 1.0);
        assert_eq!(r.variance[1], 1.0);
    }

    #[test]
    fn median_geometry_uses_equal_weights_not_noise() {
        // Median ignores per-frame weights, so the geometry planes must too. With 3 frames and
        // Noise weighting, equal weights give weight = 3 (frame count) and variance = 3/3² = 1/3;
        // noise weights would (mis)report a normalized Σw ≈ 1 instead.
        let dims = ImageDimensions::new((8, 1), 1);
        let mk = |base: f32, spread: f32| -> Vec<f32> {
            (0..8).map(|i| base + i as f32 * spread / 7.0).collect()
        };
        let frames: Vec<StackFrame> = vec![
            AstroImage::from_pixels(dims, mk(100.0, 1.0)).into(),
            AstroImage::from_pixels(dims, mk(100.0, 20.0)).into(),
            AstroImage::from_pixels(dims, mk(100.0, 2.0)).into(),
        ];
        let config = StackConfig {
            method: CombineMethod::Median,
            weighting: Weighting::Noise,
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
        for p in 0..8 {
            assert_eq!(r.coverage[p], 1.0);
            assert_eq!(
                r.weight[p], 3.0,
                "median geometry must use equal weights (= 3)"
            );
            assert!(
                (r.variance[p] - 1.0 / 3.0).abs() < 1e-6,
                "variance = {}",
                r.variance[p]
            );
        }
    }

    // ========== Normalization: Identity ==========

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

    // ========== Global Normalization ==========

    #[test]
    fn test_global_norm_offset_correction() {
        // Same scale, different median -> gain ~1.0, offset ~-100
        let dims = ImageDimensions::new((4, 4), 1);
        let frame0: Vec<f32> = (0..16).map(|i| 100.0 + i as f32).collect();
        let frame1: Vec<f32> = (0..16).map(|i| 200.0 + i as f32).collect();
        let cache = make_test_cache(vec![
            AstroImage::from_pixels(dims, frame0),
            AstroImage::from_pixels(dims, frame1),
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
            AstroImage::from_pixels(dims, frame0),
            AstroImage::from_pixels(dims, frame1),
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

        let result = cache.process_chunked_weighted(None, Some(&norm_params), |values, _, _| {
            math::sum::mean_f32(values)
        });
        assert_channel_near(&result, 0, 100.0, 1.0);
    }

    // ========== Multiplicative Normalization ==========

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
            AstroImage::from_pixels(dims, frame0),
            AstroImage::from_pixels(dims, frame1),
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

        let result = cache.process_chunked_weighted(None, Some(&norm_params), |values, _, _| {
            math::sum::mean_f32(values)
        });
        assert_channel_near(&result, 0, 100.0, 1.0);
    }

    // ========== RGB Normalization ==========

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
                cache.process_chunked_weighted(None, Some(&norm_params), |values, _, _| {
                    math::sum::mean_f32(values)
                });

            for (ch, &expected) in ref_rgb.iter().enumerate() {
                assert_channel_near(&result, ch, expected, 2.0);
            }
        }
    }

    // ========== Dispatch ==========

    #[test]
    fn test_dispatch_normalized_vs_unnormalized() {
        let cache = make_uniform_frames(16, &[100.0, 200.0]);
        let norm_params = norm_params_for(&cache, Normalization::Global).unwrap();

        let result_norm =
            cache.process_chunked_weighted(None, Some(&norm_params), |values, w, scratch| {
                Rejection::None.combine_mean(values, w, scratch)
            });
        let result_unnorm = cache.process_chunked_weighted(None, None, |values, w, scratch| {
            Rejection::None.combine_mean(values, w, scratch)
        });

        let norm_pixel = result_norm.channel(0)[0];
        let unnorm_pixel = result_unnorm.channel(0)[0];
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
        let temp_dir = std::env::temp_dir().join("lumos_disk_stack_combine_test");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let dims = ImageDimensions::new((4, 4), 1);
        let mut paths = Vec::new();
        for (i, &v) in [10.0f32, 20.0, 30.0].iter().enumerate() {
            let image = AstroImage::from_pixels(dims, vec![v; 16]);
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

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    // ========== Auto Reference Frame Selection ==========

    #[test]
    fn test_select_reference_frame_picks_lowest_noise() {
        // 3 frames, 1 channel: MADs are 2.0, 0.5, 1.0
        // Frame 1 (MAD=0.5) should be selected.
        use crate::stacking::combine::stack::{ChannelStats, FrameStats};
        let s = |median, mad| ChannelStats { median, mad };
        let fs = |median, mad| FrameStats {
            channels: [s(median, mad)].into_iter().collect(),
        };
        let stats = vec![
            fs(100.0, 2.0), // frame 0: high noise
            fs(100.0, 0.5), // frame 1: lowest noise
            fs(100.0, 1.0), // frame 2: medium noise
        ];
        assert_eq!(select_reference_frame(&stats), 1);
    }

    #[test]
    fn test_select_reference_frame_rgb_averages_channels() {
        // 2 frames, 3 channels. Frame 0 has lower MAD in R/G but higher in B.
        // Frame 0: MADs = [1.0, 1.0, 5.0] → avg = 2.333
        // Frame 1: MADs = [2.0, 2.0, 2.0] → avg = 2.000
        // Frame 1 should be selected (lower average).
        use crate::stacking::combine::stack::{ChannelStats, FrameStats};
        let s = |median, mad| ChannelStats { median, mad };
        let stats = vec![
            FrameStats {
                channels: [s(100.0, 1.0), s(100.0, 1.0), s(100.0, 5.0)]
                    .into_iter()
                    .collect(),
            },
            FrameStats {
                channels: [s(100.0, 2.0), s(100.0, 2.0), s(100.0, 2.0)]
                    .into_iter()
                    .collect(),
            },
        ];
        assert_eq!(select_reference_frame(&stats), 1);
    }

    #[test]
    fn test_select_reference_frame_single_frame() {
        use crate::stacking::combine::stack::{ChannelStats, FrameStats};
        let stats = vec![FrameStats {
            channels: [ChannelStats {
                median: 50.0,
                mad: 3.0,
            }]
            .into_iter()
            .collect(),
        }];
        assert_eq!(select_reference_frame(&stats), 0);
    }

    #[test]
    fn test_select_reference_frame_equal_noise() {
        // All frames have same MAD → picks first (frame 0).
        use crate::stacking::combine::stack::{ChannelStats, FrameStats};
        let s = |median, mad| ChannelStats { median, mad };
        let fs = |median, mad| FrameStats {
            channels: [s(median, mad)].into_iter().collect(),
        };
        let stats = vec![fs(100.0, 1.5), fs(200.0, 1.5), fs(300.0, 1.5)];
        assert_eq!(select_reference_frame(&stats), 0);
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
            AstroImage::from_pixels(dims, f0),
            AstroImage::from_pixels(dims, f1),
            AstroImage::from_pixels(dims, f2),
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
            AstroImage::from_pixels(dims, f0),
            AstroImage::from_pixels(dims, f1),
        ]);
        let norm_params = norm_params_for(&cache, Normalization::Global).unwrap();

        // Frame 1 is reference (lower noise), so stacked result should be ~200
        let result = cache.process_chunked_weighted(None, Some(&norm_params), |values, _, _| {
            math::sum::mean_f32(values)
        });
        assert_channel_near(&result, 0, 200.0, 2.0);
    }

    // ========== Noise Weighting ==========

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
            AstroImage::from_pixels(dims, f0),
            AstroImage::from_pixels(dims, f1),
        ]);

        let stats = cache.core.channel_stats.clone();
        // sigma0 ≈ MAD*1.4826 (small), sigma1 ≈ MAD*1.4826 (large)
        let sigma0 = math::statistics::mad_to_sigma(stats[0].channels[0].mad);
        let sigma1 = math::statistics::mad_to_sigma(stats[1].channels[0].mad);
        assert!(
            sigma1 > sigma0 * 5.0,
            "Frame 1 should be much noisier: sigma0={sigma0}, sigma1={sigma1}"
        );

        let weights = resolve_weights(&Weighting::Noise, &stats, None).unwrap();
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
        let stats = cache.core.channel_stats.clone();
        // All MADs are 0 for uniform frames → all weights are 0 → returns None
        let weights = resolve_weights(&Weighting::Noise, &stats, None);
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
            AstroImage::from_pixels(dims, make_frame(100.0)),
            AstroImage::from_pixels(dims, make_frame(200.0)),
            AstroImage::from_pixels(dims, make_frame(300.0)),
        ]);
        let stats = cache.core.channel_stats.clone();
        let weights = resolve_weights(&Weighting::Noise, &stats, None).unwrap();
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
            AstroImage::from_pixels(dims, f0),
            AstroImage::from_pixels(dims, f1),
            AstroImage::from_pixels(dims, f2),
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
            FrameStats { channels }
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
