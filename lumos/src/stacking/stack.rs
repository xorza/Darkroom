//! Unified stacking entry point.
//!
//! Provides `stack()` and `stack_with_progress()` functions as the main API
//! for image stacking operations.

use std::path::Path;

use crate::AstroImage;

use super::FrameType;
use super::cache::{ImageCache, StackableImage};
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
///     rejection: Rejection::sigma_clip_asymmetric(2.0, 3.0),
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
        rejection = ?config.rejection,
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

/// Compute per-frame normalization parameters based on the normalization mode.
///
/// Uses frame 0 as the reference. Returns `None` for `Normalization::None`.
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
    let mut params = vec![NormParams::IDENTITY; frame_count * channels];

    for channel in 0..channels {
        let (ref_median, ref_mad) = stats[channel]; // frame 0

        // Frame 0 is the reference — already initialized to IDENTITY
        for frame_idx in 1..frame_count {
            let (frame_median, frame_mad) = stats[frame_idx * channels + channel];

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
        ?normalization,
        "Computed normalization (reference frame 0)"
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
        CombineMethod::Median => {
            cache.process_chunked(None, norm_params, move |values, _, scratch| {
                let remaining = config.rejection.reject(values, scratch);
                math::median_f32_mut(&mut values[..remaining])
            })
        }

        CombineMethod::Mean => {
            let weights = if config.weights.is_empty() {
                None
            } else {
                Some(config.normalized_weights())
            };

            cache.process_chunked(
                weights.as_deref(),
                norm_params,
                move |values, w, scratch| config.rejection.combine_mean(values, w, scratch).value,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stacking::config::Rejection;
    use crate::{
        astro_image::{AstroImage, ImageDimensions},
        stacking::cache::tests::make_test_cache,
    };
    use std::path::PathBuf;

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

    // ========== Global Normalization Tests ==========

    #[test]
    fn test_compute_global_norm_params_identity_for_identical_frames() {
        // All frames have same pixel values -> normalization should be identity
        let dims = ImageDimensions::new(4, 4, 1);
        let images = vec![
            AstroImage::from_pixels(dims, vec![5.0; 16]),
            AstroImage::from_pixels(dims, vec![5.0; 16]),
            AstroImage::from_pixels(dims, vec![5.0; 16]),
        ];
        let cache = make_test_cache(images);
        let params = compute_norm_params(&cache, Normalization::Global).unwrap();

        // 3 frames * 1 channel = 3 entries
        assert_eq!(params.len(), 3);
        for np in &params {
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

    #[test]
    fn test_compute_global_norm_params_offset_correction() {
        // Frame 0: values around 100, Frame 1: values around 200 (same scale, different median)
        let dims = ImageDimensions::new(4, 4, 1);
        let frame0: Vec<f32> = (0..16).map(|i| 100.0 + i as f32).collect();
        let frame1: Vec<f32> = (0..16).map(|i| 200.0 + i as f32).collect();

        let images = vec![
            AstroImage::from_pixels(dims, frame0),
            AstroImage::from_pixels(dims, frame1),
        ];
        let cache = make_test_cache(images);
        let params = compute_norm_params(&cache, Normalization::Global).unwrap();

        // Frame 0 is reference -> (gain=1, offset=0)
        assert!((params[0].gain - 1.0).abs() < 1e-5);
        assert!(params[0].offset.abs() < 1e-5);

        // Frame 1 should have gain ~1.0 (same scale), offset ~-100.0
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
    fn test_compute_global_norm_params_scale_correction() {
        // Frame 0: values in [90..110], Frame 1: values in [80..120] (wider spread)
        let dims = ImageDimensions::new(10, 10, 1);
        let frame0: Vec<f32> = (0..100).map(|i| 90.0 + (i as f32) * 20.0 / 99.0).collect();
        let frame1: Vec<f32> = (0..100).map(|i| 80.0 + (i as f32) * 40.0 / 99.0).collect();

        let images = vec![
            AstroImage::from_pixels(dims, frame0),
            AstroImage::from_pixels(dims, frame1),
        ];
        let cache = make_test_cache(images);
        let params = compute_norm_params(&cache, Normalization::Global).unwrap();

        // Frame 1 has ~2x the spread of frame 0, so gain should be ~0.5
        assert!(
            (params[1].gain - 0.5).abs() < 0.15,
            "Gain should be ~0.5 (narrowing wider frame), got {}",
            params[1].gain
        );
    }

    #[test]
    fn test_normalized_mean_stacking_corrects_offset() {
        // Two frames with different brightness but same structure.
        // Without normalization, mean would blend the two levels.
        // With normalization, both should be brought to the reference level.
        let dims = ImageDimensions::new(4, 4, 1);
        let base_value = 100.0;
        let offset = 50.0;
        let frame0: Vec<f32> = vec![base_value; 16];
        let frame1: Vec<f32> = vec![base_value + offset; 16];

        let images_norm = vec![
            AstroImage::from_pixels(dims, frame0.clone()),
            AstroImage::from_pixels(dims, frame1.clone()),
        ];
        let cache = make_test_cache(images_norm);
        let norm_params = compute_norm_params(&cache, Normalization::Global).unwrap();

        // With normalization: all pixels should be close to base_value
        let result = cache.process_chunked(None, Some(&norm_params), |values, _, _| {
            math::mean_f32(values)
        });
        for &pixel in result.channel(0).pixels() {
            assert!(
                (pixel - base_value).abs() < 1.0,
                "Normalized mean should be ~{}, got {}",
                base_value,
                pixel
            );
        }

        // Without normalization: mean would be base_value + offset/2 = 125
        let images_raw = vec![
            AstroImage::from_pixels(dims, frame0),
            AstroImage::from_pixels(dims, frame1),
        ];
        let cache_raw = make_test_cache(images_raw);
        let result_raw =
            cache_raw.process_chunked(None, None, |values, _, _| math::mean_f32(values));
        for &pixel in result_raw.channel(0).pixels() {
            assert!(
                (pixel - (base_value + offset / 2.0)).abs() < 1.0,
                "Unnormalized mean should be ~{}, got {}",
                base_value + offset / 2.0,
                pixel
            );
        }
    }

    #[test]
    fn test_normalized_stacking_rgb() {
        // RGB frames where each channel has a different offset in frame 1
        let dims = ImageDimensions::new(4, 4, 3);
        // Frame 0: R=100, G=200, B=300 for all pixels
        let pixels0: Vec<f32> = (0..16).flat_map(|_| vec![100.0, 200.0, 300.0]).collect();
        // Frame 1: R=120, G=180, B=350 for all pixels
        let pixels1: Vec<f32> = (0..16).flat_map(|_| vec![120.0, 180.0, 350.0]).collect();

        let images = vec![
            AstroImage::from_pixels(dims, pixels0),
            AstroImage::from_pixels(dims, pixels1),
        ];
        let cache = make_test_cache(images);
        let norm_params = compute_norm_params(&cache, Normalization::Global).unwrap();

        let result = cache.process_chunked(None, Some(&norm_params), |values, _, _| {
            math::mean_f32(values)
        });

        // After normalization to frame 0's reference, each channel should be
        // close to the reference frame's values
        for &pixel in result.channel(0).pixels() {
            assert!(
                (pixel - 100.0).abs() < 2.0,
                "R channel should be ~100, got {}",
                pixel
            );
        }
        for &pixel in result.channel(1).pixels() {
            assert!(
                (pixel - 200.0).abs() < 2.0,
                "G channel should be ~200, got {}",
                pixel
            );
        }
        for &pixel in result.channel(2).pixels() {
            assert!(
                (pixel - 300.0).abs() < 2.0,
                "B channel should be ~300, got {}",
                pixel
            );
        }
    }

    #[test]
    fn test_dispatch_normalized_vs_unnormalized() {
        // Verify dispatch_stacking applies normalization when params present
        let dims = ImageDimensions::new(4, 4, 1);
        let frame0: Vec<f32> = vec![100.0; 16];
        let frame1: Vec<f32> = vec![200.0; 16];

        let images = vec![
            AstroImage::from_pixels(dims, frame0.clone()),
            AstroImage::from_pixels(dims, frame1.clone()),
        ];
        let cache = make_test_cache(images);
        let norm_params = compute_norm_params(&cache, Normalization::Global).unwrap();

        let config = StackConfig {
            normalization: Normalization::Global,
            rejection: Rejection::None,
            ..Default::default()
        };

        let result_norm = dispatch_stacking(&cache, &config, Some(&norm_params));
        let result_unnorm = dispatch_stacking(&cache, &config, None);

        // Normalized: should be ~100 (reference frame level)
        // Unnormalized: should be ~150 (average of 100 and 200)
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

    // ========== Multiplicative Normalization Tests ==========

    #[test]
    fn test_multiplicative_norm_identity_for_identical_frames() {
        let dims = ImageDimensions::new(4, 4, 1);
        let images = vec![
            AstroImage::from_pixels(dims, vec![5.0; 16]),
            AstroImage::from_pixels(dims, vec![5.0; 16]),
            AstroImage::from_pixels(dims, vec![5.0; 16]),
        ];
        let cache = make_test_cache(images);
        let params = compute_norm_params(&cache, Normalization::Multiplicative).unwrap();

        assert_eq!(params.len(), 3);
        for np in &params {
            assert!(
                (np.gain - 1.0).abs() < 1e-5,
                "Gain should be ~1.0, got {}",
                np.gain
            );
            assert!(
                np.offset.abs() < 1e-5,
                "Offset should be 0.0, got {}",
                np.offset
            );
        }
    }

    #[test]
    fn test_multiplicative_norm_scales_by_median_ratio() {
        // Simulate flat frames: frame 0 median ~100, frame 1 median ~200
        let dims = ImageDimensions::new(4, 4, 1);
        let frame0: Vec<f32> = vec![100.0; 16];
        let frame1: Vec<f32> = vec![200.0; 16];

        let images = vec![
            AstroImage::from_pixels(dims, frame0),
            AstroImage::from_pixels(dims, frame1),
        ];
        let cache = make_test_cache(images);
        let params = compute_norm_params(&cache, Normalization::Multiplicative).unwrap();

        // Frame 0: reference -> gain=1, offset=0
        assert!((params[0].gain - 1.0).abs() < 1e-5);
        assert!(params[0].offset.abs() < 1e-5);

        // Frame 1: gain = 100/200 = 0.5, offset = 0
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
        // Multiplicative should only scale, never shift.
        // Frame with different median should be scaled but not shifted.
        let dims = ImageDimensions::new(10, 10, 1);
        let frame0: Vec<f32> = (0..100).map(|i| 90.0 + (i as f32) * 0.2).collect();
        let frame1: Vec<f32> = (0..100).map(|i| 180.0 + (i as f32) * 0.4).collect();

        let images = vec![
            AstroImage::from_pixels(dims, frame0),
            AstroImage::from_pixels(dims, frame1),
        ];
        let cache = make_test_cache(images);
        let params = compute_norm_params(&cache, Normalization::Multiplicative).unwrap();

        // All offsets must be exactly 0
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
        // Two flat frames with different exposure levels.
        // After multiplicative normalization, stacked result should match reference level.
        let dims = ImageDimensions::new(4, 4, 1);
        let frame0: Vec<f32> = vec![100.0; 16];
        let frame1: Vec<f32> = vec![200.0; 16];

        let images = vec![
            AstroImage::from_pixels(dims, frame0),
            AstroImage::from_pixels(dims, frame1),
        ];
        let cache = make_test_cache(images);
        let norm_params = compute_norm_params(&cache, Normalization::Multiplicative).unwrap();

        let result = cache.process_chunked(None, Some(&norm_params), |values, _, _| {
            math::mean_f32(values)
        });

        // Both frames scaled to reference level (100.0), mean should be 100.0
        for &pixel in result.channel(0).pixels() {
            assert!(
                (pixel - 100.0).abs() < 1.0,
                "Multiplicative stacked flat should be ~100, got {}",
                pixel
            );
        }
    }

    #[test]
    fn test_multiplicative_rgb() {
        // RGB flat frames with per-channel exposure differences
        let dims = ImageDimensions::new(4, 4, 3);
        let pixels0: Vec<f32> = (0..16).flat_map(|_| vec![100.0, 200.0, 300.0]).collect();
        let pixels1: Vec<f32> = (0..16).flat_map(|_| vec![150.0, 100.0, 600.0]).collect();

        let images = vec![
            AstroImage::from_pixels(dims, pixels0),
            AstroImage::from_pixels(dims, pixels1),
        ];
        let cache = make_test_cache(images);
        let norm_params = compute_norm_params(&cache, Normalization::Multiplicative).unwrap();

        let result = cache.process_chunked(None, Some(&norm_params), |values, _, _| {
            math::mean_f32(values)
        });

        for &pixel in result.channel(0).pixels() {
            assert!(
                (pixel - 100.0).abs() < 1.0,
                "R should be ~100, got {}",
                pixel
            );
        }
        for &pixel in result.channel(1).pixels() {
            assert!(
                (pixel - 200.0).abs() < 1.0,
                "G should be ~200, got {}",
                pixel
            );
        }
        for &pixel in result.channel(2).pixels() {
            assert!(
                (pixel - 300.0).abs() < 1.0,
                "B should be ~300, got {}",
                pixel
            );
        }
    }
}
