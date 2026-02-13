//! Unified stacking entry point.
//!
//! Provides `stack()` and `stack_with_progress()` functions as the main API
//! for image stacking operations.

use std::path::Path;

use crate::AstroImage;

use super::FrameType;
use super::cache::{ChannelStats, ImageCache, StackableImage};
use super::config::{CombineMethod, Normalization, StackConfig};
use super::error::Error;
use super::progress::ProgressCallback;
use crate::math;

/// Per-frame, per-channel affine normalization parameters.
///
/// Applied as `normalized = raw * gain + offset`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct NormParams {
    pub gain: f32,
    pub offset: f32,
}

impl NormParams {
    pub const IDENTITY: NormParams = NormParams {
        gain: 1.0,
        offset: 0.0,
    };
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

    // Validate weights if provided
    if !config.weights.is_empty() && config.weights.len() != paths.len() {
        panic!(
            "Weight count ({}) must match frame count ({})",
            config.weights.len(),
            paths.len()
        );
    }

    // Log stacking parameters
    tracing::info!(
        frame_type = %frame_type,
        method = ?config.method,
        normalization = ?config.normalization,
        frame_count = paths.len(),
        has_weights = !config.weights.is_empty(),
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
fn select_reference_frame(stats: &[ChannelStats], frame_count: usize, channels: usize) -> usize {
    assert!(frame_count > 0);
    let mut best_frame = 0;
    let mut best_mad = f32::MAX;

    for frame_idx in 0..frame_count {
        let mut mad_sum = 0.0f32;
        for channel in 0..channels {
            mad_sum += stats[frame_idx * channels + channel].mad;
        }
        let avg_mad = mad_sum / channels as f32;
        if avg_mad < best_mad {
            best_mad = avg_mad;
            best_frame = frame_idx;
        }
    }

    best_frame
}

/// Compute per-frame normalization parameters based on the normalization mode.
///
/// Auto-selects the lowest-noise frame as reference. Returns `None` for
/// `Normalization::None`.
///
/// - **Global**: `gain = ref_mad / frame_mad`, `offset = ref_median - frame_median * gain`
/// - **Multiplicative**: `gain = ref_median / frame_median`, `offset = 0`
fn compute_norm_params(
    cache: &ImageCache<impl StackableImage>,
    normalization: Normalization,
) -> Option<Vec<NormParams>> {
    if normalization == Normalization::None {
        return None;
    }

    let stats = cache.compute_channel_stats();
    let channels = cache.dimensions().channels;
    let frame_count = cache.frame_count();
    let ref_frame = select_reference_frame(&stats, frame_count, channels);
    let mut params = vec![NormParams::IDENTITY; frame_count * channels];

    for channel in 0..channels {
        let ChannelStats {
            median: ref_median,
            mad: ref_mad,
        } = stats[ref_frame * channels + channel];

        for frame_idx in 0..frame_count {
            if frame_idx == ref_frame {
                continue; // Reference frame keeps IDENTITY
            }

            let ChannelStats {
                median: frame_median,
                mad: frame_mad,
            } = stats[frame_idx * channels + channel];

            let np = match normalization {
                Normalization::Global => {
                    let gain = if frame_mad > f32::EPSILON {
                        ref_mad / frame_mad
                    } else {
                        1.0
                    };
                    NormParams {
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
                    NormParams { gain, offset: 0.0 }
                }
                Normalization::None => unreachable!(),
            };

            params[frame_idx * channels + channel] = np;
        }
    }

    tracing::info!(
        frame_count,
        channels,
        ref_frame,
        ?normalization,
        "Computed normalization"
    );

    Some(params)
}

/// Run the full stacking pipeline: compute normalization, dispatch combining,
/// and construct the output image.
///
/// Generic over any `StackableImage` type.
pub(crate) fn run_stacking<I: StackableImage>(cache: &ImageCache<I>, config: &StackConfig) -> I {
    let norm_params = compute_norm_params(cache, config.normalization);
    let pixels = dispatch_stacking(cache, config, norm_params.as_deref());
    I::from_stacked(pixels, cache.metadata().clone(), cache.dimensions())
}

/// Stacking dispatch that works with any `StackableImage` type.
///
/// Returns `PixelData` with the combined result.
fn dispatch_stacking(
    cache: &ImageCache<impl StackableImage>,
    config: &StackConfig,
    norm_params: Option<&[NormParams]>,
) -> crate::astro_image::PixelData {
    match config.method {
        CombineMethod::Median => cache.process_chunked(None, norm_params, |values, _, _| {
            math::median_f32_mut(values)
        }),

        CombineMethod::Mean(rejection) => {
            let weights = if config.weights.is_empty() {
                None
            } else {
                Some(config.normalized_weights())
            };

            cache.process_chunked(
                weights.as_deref(),
                norm_params,
                move |values, w, scratch| rejection.combine_mean(values, w, scratch),
            )
        }
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

    fn assert_norm_identity(params: &[NormParams]) {
        for np in params {
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
        let config = StackConfig::weighted(vec![1.0, 2.0]); // Wrong count
        let _ = stack(&paths, FrameType::Light, config);
    }

    // ========== Normalization: Identity ==========

    #[test]
    fn test_norm_identity_for_identical_frames() {
        // Both Global and Multiplicative should produce identity for identical frames
        let cache = make_uniform_frames(16, &[5.0, 5.0, 5.0]);

        for mode in [Normalization::Global, Normalization::Multiplicative] {
            let params = compute_norm_params(&cache, mode).unwrap();
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
        let params = compute_norm_params(&cache, Normalization::Global).unwrap();

        assert_norm_identity(&params[..1]);
        assert!(
            (params[1].gain - 1.0).abs() < 0.1,
            "Gain should be ~1.0, got {}",
            params[1].gain
        );
        assert!(
            (params[1].offset - (-100.0)).abs() < 1.0,
            "Offset should be ~-100.0, got {}",
            params[1].offset
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
        let params = compute_norm_params(&cache, Normalization::Global).unwrap();

        assert!(
            (params[1].gain - 0.5).abs() < 0.15,
            "Gain should be ~0.5, got {}",
            params[1].gain
        );
    }

    #[test]
    fn test_global_norm_stacking_corrects_offset() {
        // After normalization, both frames should be brought to reference level
        let cache = make_uniform_frames(16, &[100.0, 150.0]);
        let norm_params = compute_norm_params(&cache, Normalization::Global).unwrap();

        let result = cache.process_chunked(None, Some(&norm_params), |values, _, _| {
            math::mean_f32(values)
        });
        assert_channel_near(&result, 0, 100.0, 1.0);
    }

    // ========== Multiplicative Normalization ==========

    #[test]
    fn test_multiplicative_norm_scales_by_median_ratio() {
        let cache = make_uniform_frames(16, &[100.0, 200.0]);
        let params = compute_norm_params(&cache, Normalization::Multiplicative).unwrap();

        assert_norm_identity(&params[..1]);
        assert!(
            (params[1].gain - 0.5).abs() < 1e-5,
            "Gain should be 0.5, got {}",
            params[1].gain
        );
        assert!(
            params[1].offset.abs() < 1e-5,
            "Offset should be 0.0, got {}",
            params[1].offset
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
        let params = compute_norm_params(&cache, Normalization::Multiplicative).unwrap();

        for np in &params {
            assert!(
                np.offset.abs() < f32::EPSILON,
                "Multiplicative offset must be 0, got {}",
                np.offset
            );
        }
    }

    #[test]
    fn test_multiplicative_stacking_normalizes_flat_levels() {
        let cache = make_uniform_frames(16, &[100.0, 200.0]);
        let norm_params = compute_norm_params(&cache, Normalization::Multiplicative).unwrap();

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
            let norm_params = compute_norm_params(&cache, mode).unwrap();

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
        let norm_params = compute_norm_params(&cache, Normalization::Global).unwrap();

        let config = StackConfig {
            method: CombineMethod::Mean(Rejection::None),
            normalization: Normalization::Global,
            ..Default::default()
        };

        let result_norm = dispatch_stacking(&cache, &config, Some(&norm_params));
        let result_unnorm = dispatch_stacking(&cache, &config, None);

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
        use super::ChannelStats;
        let s = |median, mad| ChannelStats { median, mad };
        let stats = vec![
            s(100.0, 2.0), // frame 0: high noise
            s(100.0, 0.5), // frame 1: lowest noise
            s(100.0, 1.0), // frame 2: medium noise
        ];
        assert_eq!(select_reference_frame(&stats, 3, 1), 1);
    }

    #[test]
    fn test_select_reference_frame_rgb_averages_channels() {
        // 2 frames, 3 channels. Frame 0 has lower MAD in R/G but higher in B.
        // Frame 0: MADs = [1.0, 1.0, 5.0] → avg = 2.333
        // Frame 1: MADs = [2.0, 2.0, 2.0] → avg = 2.000
        // Frame 1 should be selected (lower average).
        use super::ChannelStats;
        let s = |median, mad| ChannelStats { median, mad };
        let stats = vec![
            s(100.0, 1.0), // frame 0, R
            s(100.0, 1.0), // frame 0, G
            s(100.0, 5.0), // frame 0, B
            s(100.0, 2.0), // frame 1, R
            s(100.0, 2.0), // frame 1, G
            s(100.0, 2.0), // frame 1, B
        ];
        assert_eq!(select_reference_frame(&stats, 2, 3), 1);
    }

    #[test]
    fn test_select_reference_frame_single_frame() {
        use super::ChannelStats;
        let stats = vec![ChannelStats {
            median: 50.0,
            mad: 3.0,
        }];
        assert_eq!(select_reference_frame(&stats, 1, 1), 0);
    }

    #[test]
    fn test_select_reference_frame_equal_noise() {
        // All frames have same MAD → picks first (frame 0).
        use super::ChannelStats;
        let s = |median, mad| ChannelStats { median, mad };
        let stats = vec![s(100.0, 1.5), s(200.0, 1.5), s(300.0, 1.5)];
        assert_eq!(select_reference_frame(&stats, 3, 1), 0);
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

        let params = compute_norm_params(&cache, Normalization::Global).unwrap();

        // Frame 1 (lowest noise) should have identity params
        assert!(
            (params[1].gain - 1.0).abs() < 1e-5,
            "Reference frame (1) should have gain=1.0, got {}",
            params[1].gain
        );
        assert!(
            params[1].offset.abs() < 1e-5,
            "Reference frame (1) should have offset=0.0, got {}",
            params[1].offset
        );

        // Frame 0 should NOT have identity (it's not the reference)
        assert!(
            (params[0].gain - 1.0).abs() > 0.01 || params[0].offset.abs() > 0.1,
            "Frame 0 should not have identity params: gain={}, offset={}",
            params[0].gain,
            params[0].offset
        );

        // Frame 2 should NOT have identity either
        assert!(
            (params[2].gain - 1.0).abs() > 0.01 || params[2].offset.abs() > 0.1,
            "Frame 2 should not have identity params: gain={}, offset={}",
            params[2].gain,
            params[2].offset
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
        let norm_params = compute_norm_params(&cache, Normalization::Global).unwrap();

        // Frame 1 is reference (lower noise), so stacked result should be ~200
        let result = cache.process_chunked(None, Some(&norm_params), |values, _, _| {
            math::mean_f32(values)
        });
        assert_channel_near(&result, 0, 200.0, 2.0);
    }
}
