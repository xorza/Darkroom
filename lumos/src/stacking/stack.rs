//! Unified stacking entry point.
//!
//! Provides `stack()` and `stack_with_progress()` functions as the main API
//! for image stacking operations.

use std::path::Path;

use arrayvec::ArrayVec;

use crate::AstroImage;

use super::FrameType;
use super::cache::{FrameStats, ImageCache, StackableImage};
use super::config::{CombineMethod, Normalization, StackConfig, Weighting};
use super::error::Error;
use super::progress::ProgressCallback;
use crate::math;
use crate::star_detection::ChannelStats;

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

/// Stack multiple images into a single result.
///
/// This is the main entry point for image stacking. It combines multiple frames
/// using the specified configuration.
///
/// # Arguments
///
/// * `paths` - Paths to input images
/// * `frame_type` - Type of frame being stacked (Light, Dark, Flat, Bias)
/// * `config` - Stacking configuration
///
/// # Returns
///
/// The stacked result as an `AstroImage`.
///
/// # Errors
///
/// Returns an error if:
/// - No paths are provided
/// - Image loading fails
/// - Image dimensions don't match
/// - Cache directory creation fails (for disk-backed storage)
///
/// # Examples
///
/// ```ignore
/// use lumos::stacking::{stack, StackConfig, FrameType};
///
/// // Default sigma-clipped mean
/// let result = stack(&paths, FrameType::Light, StackConfig::default())?;
///
/// // Median stacking
/// let result = stack(&paths, FrameType::Light, StackConfig::median())?;
///
/// // Custom configuration
/// let config = StackConfig {
///     method: CombineMethod::Mean(Rejection::sigma_clip_asymmetric(2.0, 3.0)),
///     ..Default::default()
/// };
/// let result = stack(&paths, FrameType::Light, config)?;
/// ```
pub fn stack<P: AsRef<Path> + Sync>(
    paths: &[P],
    frame_type: FrameType,
    config: StackConfig,
) -> Result<AstroImage, Error> {
    stack_with_progress(paths, frame_type, config, ProgressCallback::default())
}

/// Stack multiple images with progress reporting.
///
/// Same as `stack()` but accepts a progress callback for monitoring.
///
/// # Arguments
///
/// * `paths` - Paths to input images
/// * `frame_type` - Type of frame being stacked
/// * `config` - Stacking configuration
/// * `progress` - Progress callback
///
/// # Examples
///
/// ```ignore
/// use lumos::stacking::{stack_with_progress, StackConfig, FrameType, ProgressCallback};
///
/// let progress = ProgressCallback::new(|stage, current, total| {
///     println!("{}: {}/{}", stage, current, total);
/// });
///
/// let result = stack_with_progress(&paths, FrameType::Light, StackConfig::default(), progress)?;
/// ```
pub fn stack_with_progress<P: AsRef<Path> + Sync>(
    paths: &[P],
    frame_type: FrameType,
    config: StackConfig,
    progress: ProgressCallback,
) -> Result<AstroImage, Error> {
    if paths.is_empty() {
        return Err(Error::NoPaths);
    }

    // Validate configuration
    config.validate();

    // Validate manual weights count
    if let Weighting::Manual(ref w) = config.weighting {
        assert_eq!(
            w.len(),
            paths.len(),
            "Weight count ({}) must match frame count ({})",
            w.len(),
            paths.len()
        );
    }

    // Log stacking parameters
    tracing::info!(
        frame_type = %frame_type,
        method = ?config.method,
        weighting = ?config.weighting,
        normalization = ?config.normalization,
        frame_count = paths.len(),
        "Starting unified stack"
    );

    // Create image cache
    let cache = ImageCache::<AstroImage>::from_paths(paths, &config.cache, frame_type, progress)?;

    // Run stacking pipeline (normalization + combining + construction)
    let result = run_stacking(&cache, &config);

    // Cleanup cache
    cache.cleanup();

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
fn resolve_weights(weighting: &Weighting, stats: &[FrameStats]) -> Option<Vec<f32>> {
    match weighting {
        Weighting::Equal => None,
        Weighting::Noise => {
            assert!(
                !stats.is_empty(),
                "channel stats required for noise weighting"
            );
            // w = 1/sigma^2 where sigma = average MAD-based sigma across channels
            let weights: Vec<f32> = stats
                .iter()
                .map(|fs| {
                    let sigma_sum: f32 =
                        fs.channels.iter().map(|c| math::mad_to_sigma(c.mad)).sum();
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

/// Normalize weights to sum to 1.0. Returns `None` if total weight is zero.
fn normalize_weights(weights: &[f32]) -> Option<Vec<f32>> {
    let sum: f32 = weights.iter().sum();
    if sum > f32::EPSILON {
        Some(weights.iter().map(|w| w / sum).collect())
    } else {
        None
    }
}

/// Run the full stacking pipeline: compute normalization, dispatch combining,
/// and construct the output image.
///
/// Generic over any `StackableImage` type.
pub(crate) fn run_stacking<I: StackableImage>(cache: &ImageCache<I>, config: &StackConfig) -> I {
    let stats = cache.channel_stats();
    let frame_norms = compute_frame_norms(stats, config.normalization);
    let weights = resolve_weights(&config.weighting, stats);

    let pixels = dispatch_stacking(cache, &weights, config, frame_norms.as_deref());
    I::from_stacked(pixels, cache.metadata().clone(), cache.dimensions())
}

/// Stacking dispatch that works with any `StackableImage` type.
///
/// Returns `PixelData` with the combined result.
fn dispatch_stacking(
    cache: &ImageCache<impl StackableImage>,
    weights: &Option<Vec<f32>>,
    config: &StackConfig,
    frame_norms: Option<&[FrameNorm]>,
) -> crate::astro_image::PixelData {
    match config.method {
        CombineMethod::Median => cache.process_chunked(None, frame_norms, |values, _, _| {
            math::median_f32_mut(values)
        }),

        CombineMethod::Mean(rejection) => cache.process_chunked(
            weights.as_deref(),
            frame_norms,
            move |values, w, scratch| rejection.combine_mean(values, w, scratch),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stacking::rejection::Rejection;
    use crate::{
        astro_image::{AstroImage, ImageDimensions},
        stacking::cache::tests::make_test_cache,
    };
    use std::path::PathBuf;

    // ========== Helpers ==========

    /// Convenience wrapper: compute stats + norm params from cache (for tests).
    fn norm_params_for(
        cache: &ImageCache<AstroImage>,
        normalization: Normalization,
    ) -> Option<Vec<FrameNorm>> {
        let stats = cache.channel_stats().to_vec();
        compute_frame_norms(&stats, normalization)
    }

    fn make_uniform_frames(pixel_counts: usize, values: &[f32]) -> ImageCache<AstroImage> {
        let dims = ImageDimensions::new(pixel_counts, 1, 1);
        let images = values
            .iter()
            .map(|&v| AstroImage::from_pixels(dims, vec![v; pixel_counts]))
            .collect();
        make_test_cache(images)
    }

    fn make_rgb_frames(pixels: usize, frame_values: &[[f32; 3]]) -> ImageCache<AstroImage> {
        let dims = ImageDimensions::new(pixels, 1, 3);
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

    fn assert_channel_near(
        result: &crate::astro_image::PixelData,
        channel: usize,
        expected: f32,
        tol: f32,
    ) {
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
        let result = stack(&paths, FrameType::Light, StackConfig::default());
        assert!(matches!(result.unwrap_err(), Error::NoPaths));
    }

    #[test]
    fn test_stack_nonexistent_file() {
        let paths = vec![PathBuf::from("/nonexistent/image.fits")];
        let result = stack(&paths, FrameType::Light, StackConfig::default());
        assert!(matches!(result.unwrap_err(), Error::ImageLoad { .. }));
    }

    #[test]
    #[should_panic(expected = "Weight count")]
    fn test_stack_weight_mismatch() {
        let paths = vec![
            PathBuf::from("/a.fits"),
            PathBuf::from("/b.fits"),
            PathBuf::from("/c.fits"),
        ];
        let mut config = StackConfig::weighted(vec![1.0, 2.0]); // Wrong count
        config.normalization = Normalization::None; // avoid loading files for stats
        let _ = stack(&paths, FrameType::Light, config);
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
        let dims = ImageDimensions::new(4, 4, 1);
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
        let dims = ImageDimensions::new(10, 10, 1);
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

        let result = cache.process_chunked(None, Some(&norm_params), |values, _, _| {
            math::mean_f32(values)
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
        let dims = ImageDimensions::new(10, 10, 1);
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

        let result = cache.process_chunked(None, Some(&norm_params), |values, _, _| {
            math::mean_f32(values)
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

            let result = cache.process_chunked(None, Some(&norm_params), |values, _, _| {
                math::mean_f32(values)
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

        let config = StackConfig {
            method: CombineMethod::Mean(Rejection::None),
            normalization: Normalization::Global,
            ..Default::default()
        };
        let no_weights = None;

        let result_norm = dispatch_stacking(&cache, &no_weights, &config, Some(&norm_params));
        let result_unnorm = dispatch_stacking(&cache, &no_weights, &config, None);

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

    // ========== Auto Reference Frame Selection ==========

    #[test]
    fn test_select_reference_frame_picks_lowest_noise() {
        // 3 frames, 1 channel: MADs are 2.0, 0.5, 1.0
        // Frame 1 (MAD=0.5) should be selected.
        use super::{ChannelStats, FrameStats};
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
        use super::{ChannelStats, FrameStats};
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
        use super::{ChannelStats, FrameStats};
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
        use super::{ChannelStats, FrameStats};
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
        let dims = ImageDimensions::new(10, 10, 1);
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
        let dims = ImageDimensions::new(16, 1, 1);
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
        let result = cache.process_chunked(None, Some(&norm_params), |values, _, _| {
            math::mean_f32(values)
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
        let dims = ImageDimensions::new(100, 1, 1);
        let f0: Vec<f32> = (0..100).map(|i| 99.75 + (i as f32) * 0.5 / 99.0).collect();
        let f1: Vec<f32> = (0..100).map(|i| 190.0 + (i as f32) * 20.0 / 99.0).collect();
        let cache = make_test_cache(vec![
            AstroImage::from_pixels(dims, f0),
            AstroImage::from_pixels(dims, f1),
        ]);

        let stats = cache.channel_stats().to_vec();
        // sigma0 ≈ MAD*1.4826 (small), sigma1 ≈ MAD*1.4826 (large)
        let sigma0 = math::mad_to_sigma(stats[0].channels[0].mad);
        let sigma1 = math::mad_to_sigma(stats[1].channels[0].mad);
        assert!(
            sigma1 > sigma0 * 5.0,
            "Frame 1 should be much noisier: sigma0={sigma0}, sigma1={sigma1}"
        );

        let weights = resolve_weights(&Weighting::Noise, &stats).unwrap();
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
        let stats = cache.channel_stats().to_vec();
        // All MADs are 0 for uniform frames → all weights are 0 → returns None
        let weights = resolve_weights(&Weighting::Noise, &stats);
        assert!(
            weights.is_none(),
            "Uniform frames have zero MAD → no weights (equal weighting fallback)"
        );
    }

    #[test]
    fn test_noise_weighting_with_spread_equal_noise() {
        // 3 frames with identical spread → equal weights
        let dims = ImageDimensions::new(100, 1, 1);
        let make_frame =
            |base: f32| -> Vec<f32> { (0..100).map(|i| base + (i as f32) * 10.0 / 99.0).collect() };
        let cache = make_test_cache(vec![
            AstroImage::from_pixels(dims, make_frame(100.0)),
            AstroImage::from_pixels(dims, make_frame(200.0)),
            AstroImage::from_pixels(dims, make_frame(300.0)),
        ]);
        let stats = cache.channel_stats().to_vec();
        let weights = resolve_weights(&Weighting::Noise, &stats).unwrap();
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
        let dims = ImageDimensions::new(16, 1, 1);
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
        let result = run_stacking(&cache, &config);
        let pixel = result.channel(0).pixels()[0];
        // Result should be near 100 (clean frame value), not near 999 (outlier)
        assert!(
            (pixel - 100.0).abs() < 10.0,
            "Noise-weighted + sigma-clip should be ~100, got {pixel}"
        );
    }

    #[test]
    fn test_manual_weighting_unchanged() {
        // Manual(vec![1.0, 2.0, 3.0]) should produce normalized [1/6, 2/6, 3/6]
        let weights = resolve_weights(&Weighting::Manual(vec![1.0, 2.0, 3.0]), &[]).unwrap();
        assert_eq!(weights.len(), 3);
        assert!((weights[0] - 1.0 / 6.0).abs() < 1e-6);
        assert!((weights[1] - 2.0 / 6.0).abs() < 1e-6);
        assert!((weights[2] - 3.0 / 6.0).abs() < 1e-6);
    }

    #[test]
    fn test_equal_weighting_returns_none() {
        let weights = resolve_weights(&Weighting::Equal, &[]);
        assert!(weights.is_none());
    }

    #[test]
    fn test_light_preset_uses_noise_weighting() {
        let config = StackConfig::light();
        assert_eq!(config.weighting, Weighting::Noise);
    }
}
