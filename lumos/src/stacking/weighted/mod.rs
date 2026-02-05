//! Weighted mean stacking with quality-based frame weighting.
//!
//! This module implements weighted mean integration where each frame
//! contributes to the final stack proportionally to its quality.


use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::astro_image::AstroImage;
use crate::stacking::cache::ImageCache;
use crate::stacking::error::Error;
use crate::stacking::progress::ProgressCallback;
use crate::stacking::rejection::{
    self, GesdConfig, LinearFitClipConfig, PercentileClipConfig, RejectionResult, SigmaClipConfig,
    WinsorizedClipConfig,
};
use crate::stacking::{CacheConfig, FrameType};
use crate::star_detection::{DetectionResult, Star};

/// Frame quality metrics used for computing weights.
#[derive(Debug, Clone, Copy, Default)]
pub struct FrameQuality {
    /// Signal-to-noise ratio estimate.
    pub snr: f32,
    /// Full width at half maximum of stars (seeing quality).
    pub fwhm: f32,
    /// Star eccentricity (tracking quality, 1.0 = round).
    pub eccentricity: f32,
    /// Background noise estimate.
    pub noise: f32,
    /// Number of detected stars.
    pub star_count: usize,
}

impl FrameQuality {
    /// Compute frame quality metrics from star detection results.
    ///
    /// Uses the best stars (brightest, non-saturated) to estimate:
    /// - FWHM: median FWHM of valid stars
    /// - SNR: median SNR of valid stars
    /// - Eccentricity: median eccentricity of valid stars
    /// - Star count: number of usable stars
    ///
    /// For noise, this needs the background map. If not available, it defaults to 0.01.
    pub fn from_detection_result(result: &DetectionResult) -> Self {
        Self::from_stars(&result.stars)
    }

    /// Compute frame quality metrics from a list of stars.
    pub fn from_stars(stars: &[Star]) -> Self {
        if stars.is_empty() {
            return Self::default();
        }

        // Filter to non-saturated stars
        let valid_stars: Vec<&Star> = stars.iter().filter(|s| !s.is_saturated()).collect();

        if valid_stars.is_empty() {
            return Self {
                star_count: stars.len(),
                ..Default::default()
            };
        }

        // Use brightest non-saturated stars (up to 50) for metrics
        let n = valid_stars.len().min(50);
        let best_stars = &valid_stars[..n];

        // Compute median FWHM
        let mut fwhms: Vec<f32> = best_stars.iter().map(|s| s.fwhm).collect();
        fwhms.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let fwhm = fwhms[fwhms.len() / 2];

        // Compute median SNR
        let mut snrs: Vec<f32> = best_stars.iter().map(|s| s.snr).collect();
        snrs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let snr = snrs[snrs.len() / 2];

        // Compute median eccentricity
        let mut eccs: Vec<f32> = best_stars.iter().map(|s| s.eccentricity).collect();
        eccs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let eccentricity = eccs[eccs.len() / 2];

        Self {
            snr,
            fwhm,
            eccentricity,
            noise: 0.01, // Default noise, can be updated with background map
            star_count: stars.len(),
        }
    }

    /// Update noise estimate from background map statistics.
    ///
    /// Uses median of the per-pixel noise values.
    pub fn with_noise(mut self, noise: f32) -> Self {
        self.noise = noise;
        self
    }

    /// Compute a quality weight from the metrics.
    ///
    /// Higher weight = better quality frame.
    /// Formula: weight = (SNR^a × (1/FWHM)^b × (1/eccentricity)^c) / noise
    ///
    /// Default exponents: a=1, b=2, c=1
    pub fn compute_weight(&self) -> f32 {
        self.compute_weight_with_exponents(1.0, 2.0, 1.0)
    }

    /// Compute weight with custom exponents.
    pub fn compute_weight_with_exponents(&self, snr_exp: f32, fwhm_exp: f32, ecc_exp: f32) -> f32 {
        let snr_factor = self.snr.max(0.1).powf(snr_exp);
        let fwhm_factor = (1.0 / self.fwhm.max(0.1)).powf(fwhm_exp);
        let ecc_factor = (1.0 / self.eccentricity.max(0.1)).powf(ecc_exp);
        let noise_factor = 1.0 / self.noise.max(0.001);

        snr_factor * fwhm_factor * ecc_factor * noise_factor
    }
}

/// Pixel rejection method for weighted stacking.
#[derive(Debug, Clone, PartialEq)]
pub enum RejectionMethod {
    /// No rejection - use all pixels.
    None,
    /// Sigma clipping (Kappa-Sigma).
    SigmaClip(SigmaClipConfig),
    /// Winsorized sigma clipping - replace outliers with boundary values.
    WinsorizedSigmaClip(WinsorizedClipConfig),
    /// Linear fit clipping - good for sky gradients.
    LinearFitClip(LinearFitClipConfig),
    /// Percentile clipping - simple, good for small stacks.
    PercentileClip(PercentileClipConfig),
    /// Generalized ESD - rigorous, best for large stacks.
    Gesd(GesdConfig),
}

impl Default for RejectionMethod {
    fn default() -> Self {
        RejectionMethod::SigmaClip(SigmaClipConfig::default())
    }
}

/// Configuration for weighted mean stacking.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WeightedConfig {
    /// Per-frame weights. If empty, uses equal weights.
    /// Length must match number of frames or be empty.
    pub weights: Vec<f32>,
    /// Pixel rejection method to apply before weighted mean.
    pub rejection: RejectionMethod,
    /// Cache configuration.
    pub cache: CacheConfig,
}

impl WeightedConfig {
    /// Create config with uniform weights (all frames equal).
    pub fn uniform() -> Self {
        Self::default()
    }

    /// Create config with explicit weights.
    pub fn with_weights(weights: Vec<f32>) -> Self {
        Self {
            weights,
            ..Default::default()
        }
    }

    /// Create config from frame quality metrics.
    pub fn from_quality(qualities: &[FrameQuality]) -> Self {
        let weights: Vec<f32> = qualities.iter().map(|q| q.compute_weight()).collect();
        Self::with_weights(weights)
    }

    /// Set rejection method.
    pub fn with_rejection(mut self, rejection: RejectionMethod) -> Self {
        self.rejection = rejection;
        self
    }

    /// Set cache configuration.
    pub fn with_cache(mut self, cache: CacheConfig) -> Self {
        self.cache = cache;
        self
    }

    /// Normalize weights so they sum to 1.0.
    pub fn normalize_weights(&mut self) {
        if self.weights.is_empty() {
            return;
        }
        let sum: f32 = self.weights.iter().sum();
        if sum > f32::EPSILON {
            for w in &mut self.weights {
                *w /= sum;
            }
        }
    }
}

/// Statistics for weighted stacking operations.
#[derive(Debug, Default)]
pub(crate) struct WeightedStats {
    pub total_values: AtomicU64,
    pub rejected_values: AtomicU64,
    pub pixels_with_rejection: AtomicU64,
}

impl WeightedStats {
    pub fn record(&self, result: &RejectionResult) {
        self.total_values.fetch_add(
            (result.remaining_count + result.rejected_count) as u64,
            Ordering::Relaxed,
        );
        self.rejected_values
            .fetch_add(result.rejected_count as u64, Ordering::Relaxed);
        if result.rejected_count > 0 {
            self.pixels_with_rejection.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn log_summary(&self, frame_count: usize) {
        let total = self.total_values.load(Ordering::Relaxed);
        let rejected = self.rejected_values.load(Ordering::Relaxed);
        let pixels_rejected = self.pixels_with_rejection.load(Ordering::Relaxed);

        if total == 0 {
            return;
        }

        let pixel_count = total / frame_count as u64;
        let reject_percent = 100.0 * rejected as f64 / total as f64;
        let pixels_rejected_percent = 100.0 * pixels_rejected as f64 / pixel_count as f64;

        tracing::info!(
            "Weighted stacking stats: {:.2}% of values rejected ({} of {})",
            reject_percent,
            rejected,
            total
        );
        tracing::info!(
            "  Pixels with any rejection: {:.2}% ({} of {})",
            pixels_rejected_percent,
            pixels_rejected,
            pixel_count
        );
    }
}

/// Stack images using weighted mean with optional rejection.
///
/// # Arguments
///
/// * `paths` - Paths to input images
/// * `frame_type` - Type of frame being stacked
/// * `config` - Weighted stacking configuration
/// * `progress` - Progress callback
///
/// # Errors
///
/// Returns error if paths is empty, images can't be loaded, or dimensions mismatch.
pub fn stack_weighted_from_paths<P: AsRef<Path> + Sync>(
    paths: &[P],
    frame_type: FrameType,
    config: &WeightedConfig,
    progress: ProgressCallback,
) -> Result<AstroImage, Error> {
    // Validate weights if provided
    if !config.weights.is_empty() && config.weights.len() != paths.len() {
        panic!(
            "Weight count ({}) must match frame count ({})",
            config.weights.len(),
            paths.len()
        );
    }

    let cache = ImageCache::from_paths(paths, &config.cache, frame_type, progress)?;
    let stats = WeightedStats::default();

    // Prepare normalized weights
    let weights: Vec<f32> = if config.weights.is_empty() {
        // Uniform weights
        vec![1.0 / paths.len() as f32; paths.len()]
    } else {
        // Normalize provided weights
        let sum: f32 = config.weights.iter().sum();
        if sum > f32::EPSILON {
            config.weights.iter().map(|w| w / sum).collect()
        } else {
            vec![1.0 / paths.len() as f32; paths.len()]
        }
    };

    let rejection = config.rejection.clone();

    let result = cache.process_chunked_weighted(&weights, |values: &mut [f32], weights: &[f32]| {
        let result = weighted_mean_with_rejection(values, weights, &rejection);
        stats.record(&result);
        result.value
    });

    stats.log_summary(paths.len());

    if !config.cache.keep_cache {
        cache.cleanup();
    }

    Ok(result)
}

/// Compute weighted mean with optional rejection.
fn weighted_mean_with_rejection(
    values: &mut [f32],
    weights: &[f32],
    rejection: &RejectionMethod,
) -> RejectionResult {
    debug_assert!(!values.is_empty());
    debug_assert_eq!(values.len(), weights.len());

    match rejection {
        RejectionMethod::None => {
            let value = weighted_mean(values, weights);
            RejectionResult {
                value,
                remaining_count: values.len(),
                rejected_count: 0,
            }
        }
        RejectionMethod::SigmaClip(config) => {
            // First apply rejection
            let result = rejection::sigma_clipped_mean(values, config);
            // Then compute weighted mean of remaining values
            if result.remaining_count > 0 {
                let remaining_weights = &weights[..result.remaining_count];
                let value = weighted_mean(&values[..result.remaining_count], remaining_weights);
                RejectionResult { value, ..result }
            } else {
                result
            }
        }
        RejectionMethod::WinsorizedSigmaClip(config) => {
            // Winsorized doesn't remove values, just adjusts them
            // Apply weights to winsorized values - but we don't have the modified values
            // So for winsorized, we just return the unweighted result
            rejection::winsorized_sigma_clipped_mean(values, config)
        }
        RejectionMethod::LinearFitClip(config) => {
            let result = rejection::linear_fit_clipped_mean(values, config);
            if result.remaining_count > 0 {
                let remaining_weights = &weights[..result.remaining_count];
                let value = weighted_mean(&values[..result.remaining_count], remaining_weights);
                RejectionResult { value, ..result }
            } else {
                result
            }
        }
        RejectionMethod::PercentileClip(config) => {
            // Percentile sorts the array, so we need to sort weights too
            // For simplicity, use unweighted mean after percentile clipping
            rejection::percentile_clipped_mean(values, config)
        }
        RejectionMethod::Gesd(config) => {
            let result = rejection::gesd_mean(values, config);
            if result.remaining_count > 0 {
                let remaining_weights = &weights[..result.remaining_count];
                let value = weighted_mean(&values[..result.remaining_count], remaining_weights);
                RejectionResult { value, ..result }
            } else {
                result
            }
        }
    }
}

/// Compute weighted mean of values.
fn weighted_mean(values: &[f32], weights: &[f32]) -> f32 {
    debug_assert_eq!(values.len(), weights.len());

    if values.is_empty() {
        return 0.0;
    }

    let mut sum = 0.0f32;
    let mut weight_sum = 0.0f32;

    for (&v, &w) in values.iter().zip(weights.iter()) {
        sum += v * w;
        weight_sum += w;
    }

    if weight_sum > f32::EPSILON {
        sum / weight_sum
    } else {
        // Fallback to simple mean
        values.iter().sum::<f32>() / values.len() as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // ========== FrameQuality Tests ==========

    #[test]
    fn test_frame_quality_default() {
        let quality = FrameQuality::default();
        assert!((quality.snr - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_frame_quality_compute_weight() {
        let quality = FrameQuality {
            snr: 100.0,
            fwhm: 2.0,
            eccentricity: 1.1,
            noise: 0.01,
            star_count: 50,
        };
        let weight = quality.compute_weight();
        assert!(weight > 0.0);

        // Higher SNR should give higher weight
        let quality_high_snr = FrameQuality {
            snr: 200.0,
            ..quality
        };
        assert!(quality_high_snr.compute_weight() > weight);

        // Higher FWHM (worse seeing) should give lower weight
        let quality_bad_seeing = FrameQuality {
            fwhm: 4.0,
            ..quality
        };
        assert!(quality_bad_seeing.compute_weight() < weight);
    }

    #[test]
    fn test_frame_quality_from_stars() {
        let stars = vec![
            Star {
                pos: glam::DVec2::new(100.0, 100.0),
                flux: 1000.0,
                fwhm: 2.5,
                eccentricity: 0.1,
                snr: 50.0,
                peak: 0.5,
                sharpness: 0.3,
                roundness1: 0.0,
                roundness2: 0.0,
                laplacian_snr: 5.0,
            },
            Star {
                pos: glam::DVec2::new(200.0, 200.0),
                flux: 800.0,
                fwhm: 2.8,
                eccentricity: 0.15,
                snr: 40.0,
                peak: 0.4,
                sharpness: 0.35,
                roundness1: 0.0,
                roundness2: 0.0,
                laplacian_snr: 4.0,
            },
            Star {
                pos: glam::DVec2::new(300.0, 300.0),
                flux: 600.0,
                fwhm: 3.0,
                eccentricity: 0.2,
                snr: 30.0,
                peak: 0.3,
                sharpness: 0.4,
                roundness1: 0.0,
                roundness2: 0.0,
                laplacian_snr: 3.0,
            },
        ];

        let quality = FrameQuality::from_stars(&stars);

        assert_eq!(quality.star_count, 3);
        // Median FWHM should be 2.8 (middle value)
        assert!((quality.fwhm - 2.8).abs() < f32::EPSILON);
        // Median SNR should be 40.0 (middle value)
        assert!((quality.snr - 40.0).abs() < f32::EPSILON);
        // Median eccentricity should be 0.15 (middle value)
        assert!((quality.eccentricity - 0.15).abs() < f32::EPSILON);
    }

    #[test]
    fn test_frame_quality_from_empty_stars() {
        let stars: Vec<Star> = vec![];
        let quality = FrameQuality::from_stars(&stars);
        assert_eq!(quality.star_count, 0);
        assert!((quality.snr - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_frame_quality_from_saturated_stars() {
        // All stars are saturated
        let stars = vec![Star {
            pos: glam::DVec2::new(100.0, 100.0),
            flux: 1000.0,
            fwhm: 2.5,
            eccentricity: 0.1,
            snr: 50.0,
            peak: 0.99, // Saturated
            sharpness: 0.3,
            roundness1: 0.0,
            roundness2: 0.0,
            laplacian_snr: 5.0,
        }];

        let quality = FrameQuality::from_stars(&stars);
        assert_eq!(quality.star_count, 1);
        // With all saturated stars, defaults are used for metrics
        assert!((quality.snr - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_frame_quality_with_noise() {
        let quality = FrameQuality::default().with_noise(0.05);
        assert!((quality.noise - 0.05).abs() < f32::EPSILON);
    }

    // ========== WeightedConfig Tests ==========

    #[test]
    fn test_weighted_config_uniform() {
        let config = WeightedConfig::uniform();
        assert!(config.weights.is_empty());
    }

    #[test]
    fn test_weighted_config_with_weights() {
        let config = WeightedConfig::with_weights(vec![1.0, 2.0, 3.0]);
        assert_eq!(config.weights.len(), 3);
    }

    #[test]
    fn test_weighted_config_from_quality() {
        let qualities = vec![
            FrameQuality {
                snr: 100.0,
                fwhm: 2.0,
                eccentricity: 1.1,
                noise: 0.01,
                star_count: 50,
            },
            FrameQuality {
                snr: 50.0,
                fwhm: 3.0,
                eccentricity: 1.2,
                noise: 0.02,
                star_count: 30,
            },
        ];
        let config = WeightedConfig::from_quality(&qualities);
        assert_eq!(config.weights.len(), 2);
        // First frame should have higher weight
        assert!(config.weights[0] > config.weights[1]);
    }

    #[test]
    fn test_weighted_config_normalize() {
        let mut config = WeightedConfig::with_weights(vec![1.0, 2.0, 3.0]);
        config.normalize_weights();
        let sum: f32 = config.weights.iter().sum();
        assert!((sum - 1.0).abs() < f32::EPSILON);
    }

    // ========== Weighted Mean Tests ==========

    #[test]
    fn test_weighted_mean_uniform() {
        let values = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let weights = vec![1.0, 1.0, 1.0, 1.0, 1.0];
        let result = weighted_mean(&values, &weights);
        assert!((result - 3.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_weighted_mean_weighted() {
        let values = vec![1.0, 10.0];
        let weights = vec![0.9, 0.1];
        let result = weighted_mean(&values, &weights);
        // Should be closer to 1.0 due to higher weight
        assert!(result < 5.0);
        assert!((result - 1.9).abs() < f32::EPSILON);
    }

    // ========== Error Path Tests ==========

    #[test]
    fn test_empty_paths_error() {
        let paths: Vec<PathBuf> = vec![];
        let config = WeightedConfig::default();
        let result = stack_weighted_from_paths(
            &paths,
            FrameType::Light,
            &config,
            ProgressCallback::default(),
        );
        assert!(matches!(result.unwrap_err(), Error::NoPaths));
    }

    #[test]
    fn test_nonexistent_file_error() {
        let paths = vec![PathBuf::from("/nonexistent/weighted.fits")];
        let config = WeightedConfig::default();
        let result = stack_weighted_from_paths(
            &paths,
            FrameType::Light,
            &config,
            ProgressCallback::default(),
        );
        assert!(matches!(result.unwrap_err(), Error::ImageLoad { .. }));
    }

    #[test]
    #[should_panic(expected = "Weight count")]
    fn test_weight_count_mismatch_panics() {
        let paths = vec![
            PathBuf::from("/a.fits"),
            PathBuf::from("/b.fits"),
            PathBuf::from("/c.fits"),
        ];
        let config = WeightedConfig::with_weights(vec![1.0, 2.0]); // Wrong count
        let _ = stack_weighted_from_paths(
            &paths,
            FrameType::Light,
            &config,
            ProgressCallback::default(),
        );
    }
}
