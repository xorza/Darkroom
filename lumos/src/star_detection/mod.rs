//! Star detection and centroid computation for image registration.
//!
//! This module detects stars in astronomical images and computes sub-pixel
//! accurate centroids for use in image alignment and stacking.
//!
//! # Algorithm Overview
//!
//! 1. **Background estimation**: Divide image into tiles, compute sigma-clipped
//!    median per tile, then bilinearly interpolate to create a smooth background map.
//!
//! 2. **Star detection**: Threshold pixels above background + k×σ, then use
//!    connected component labeling to group pixels into candidate stars.
//!
//! 3. **Filtering**: Reject candidates that are too small, too large, elongated,
//!    near edges, or saturated.
//!
//! 4. **Sub-pixel centroid**: Compute precise centroid using iterative weighted
//!    centroid algorithm (achieves ~0.05 pixel accuracy).
//!
//! 5. **Quality metrics**: Compute FWHM, SNR, and eccentricity for each star.

pub(crate) mod background;
mod centroid;
pub(crate) mod common;
mod config;
mod convolution;
mod cosmic_ray;
mod deblend;
mod defect_map;
pub(crate) mod detection;
pub mod gpu;
mod median_filter;
mod star;

#[cfg(test)]
pub mod tests;

// Public API exports - main entry points for external consumers
pub use centroid::LocalBackgroundMethod;

// Internal re-exports for advanced users (may change in future versions)
// Background estimation (used by calibration and advanced pipelines)
pub use background::{BackgroundConfig, BackgroundMap};

// Profile fitting (for custom centroiding pipelines)
pub use centroid::{GaussianFitConfig, GaussianFitResult, fit_gaussian_2d};
pub use centroid::{
    MoffatFitConfig, MoffatFitResult, alpha_beta_to_fwhm, fit_moffat_2d, fwhm_beta_to_alpha,
};

// Low-level detection (for custom pipelines)
pub(crate) use centroid::compute_centroid;
pub(crate) use convolution::{matched_filter, matched_filter_elliptical};
pub(crate) use detection::{detect_stars, detect_stars_filtered};
pub(crate) use median_filter::median_filter_3x3;

// Configuration types
pub use config::StarDetectionConfig;

// Star type
pub use star::Star;

use crate::astro_image::AstroImage;
use crate::common::Buffer2;

/// Method for computing sub-pixel centroids.
///
/// Different methods offer tradeoffs between accuracy and speed.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum CentroidMethod {
    /// Iterative weighted centroid using Gaussian weights.
    /// Fast (~0.05 pixel accuracy). This is the default.
    #[default]
    WeightedMoments,

    /// 2D Gaussian profile fitting via Levenberg-Marquardt optimization.
    /// High precision (~0.01 pixel accuracy) but ~8x slower than WeightedMoments.
    /// Best for well-sampled, symmetric PSFs.
    GaussianFit,

    /// 2D Moffat profile fitting with configurable beta parameter.
    /// High precision (~0.01 pixel accuracy), similar speed to GaussianFit.
    /// Better model for atmospheric seeing (extended wings).
    /// Beta parameter controls wing slope: 2.5 typical for ground-based, 4.5 for space-based.
    MoffatFit {
        /// Power law slope controlling wing falloff. Typical range: 2.0-5.0.
        /// Lower values = more extended wings.
        beta: f32,
    },
}

pub use defect_map::DefectMap;

/// Star detector with builder pattern for convenient star detection.
///
/// Wraps [`StarDetectionConfig`] and provides methods for detecting stars
/// in single images or batches.
///
/// # Example
///
/// ```rust,ignore
/// use lumos::{StarDetector, AstroImage};
///
/// // Simple usage with defaults
/// let detector = StarDetector::new();
/// let result = detector.detect(&image);
///
/// // With custom configuration
/// let detector = StarDetector::new()
///     .with_fwhm(4.0)
///     .with_min_snr(15.0)
///     .with_edge_margin(20)
///     .build();
/// let result = detector.detect(&image);
///
/// // Batch detection (parallel)
/// let results = detector.detect_all(&images);
/// ```
#[derive(Debug, Default)]
pub struct StarDetector {
    config: StarDetectionConfig,
}

impl StarDetector {
    /// Create a new star detector with default configuration.
    pub fn new() -> Self {
        Self {
            config: StarDetectionConfig::default(),
        }
    }

    /// Create a star detector from an existing configuration.
    pub fn from_config(config: StarDetectionConfig) -> Self {
        Self { config }
    }

    /// Get reference to the underlying configuration.
    pub fn config(&self) -> &StarDetectionConfig {
        &self.config
    }

    /// Detect stars in a single image.
    pub fn detect(&self, image: &AstroImage) -> StarDetectionResult {
        self.config.validate();

        let width = image.width();
        let height = image.height();
        let mut pixels = image.to_grayscale_buffer();
        let mut output = Buffer2::new_default(width, height);

        // Step 0a: Apply defect mask if provided
        if let Some(defect_map) = self.config.defect_map.as_ref()
            && !defect_map.is_empty()
        {
            defect_map.apply(&pixels, &mut output);
            std::mem::swap(&mut pixels, &mut output);
        }

        // Step 0b: Apply 3x3 median filter to remove Bayer pattern artifacts
        // Only applied for CFA sensors; skip for monochrome (~6ms faster on 4K images)
        if image.metadata.is_cfa {
            median_filter_3x3(&pixels, &mut output);
            std::mem::swap(&mut pixels, &mut output);
        }

        drop(output);

        // Step 1: Estimate background
        let background = self.config.background_config.estimate(&pixels);

        // Step 2: Detect star candidates
        let candidates = {
            if self.config.expected_fwhm > f32::EPSILON {
                // Apply matched filter (Gaussian convolution) for better faint star detection
                // This is the DAOFIND/SExtractor technique
                let filtered = if self.config.psf_axis_ratio < 0.99 {
                    tracing::debug!(
                        "Applying elliptical matched filter with FWHM={:.1}, axis_ratio={:.2}, angle={:.2}",
                        self.config.expected_fwhm,
                        self.config.psf_axis_ratio,
                        self.config.psf_angle
                    );
                    matched_filter_elliptical(
                        &pixels,
                        &background.background,
                        self.config.expected_fwhm,
                        self.config.psf_axis_ratio,
                        self.config.psf_angle,
                    )
                } else {
                    tracing::debug!(
                        "Applying matched filter with FWHM={:.1} pixels",
                        self.config.expected_fwhm
                    );
                    matched_filter(&pixels, &background.background, self.config.expected_fwhm)
                };
                detect_stars_filtered(&pixels, &filtered, &background, &self.config)
            } else {
                // No matched filter - use standard detection
                detect_stars(&pixels, &background, &self.config)
            }
        };

        let mut diagnostics = StarDetectionDiagnostics {
            candidates_after_filtering: candidates.len(),
            ..Default::default()
        };
        tracing::debug!("Detected {} star candidates", candidates.len());

        // Step 3: Compute precise centroids
        let mut stars: Vec<Star> = {
            candidates
                .into_iter()
                .filter_map(|candidate| {
                    compute_centroid(&pixels, &background, &candidate, &self.config)
                })
                .collect()
        };
        diagnostics.stars_after_centroid = stars.len();

        // Step 4: Apply quality filters and count rejections
        stars.retain(|star| {
            if star.is_saturated() {
                diagnostics.rejected_saturated += 1;
                false
            } else if star.snr < self.config.min_snr {
                diagnostics.rejected_low_snr += 1;
                false
            } else if star.eccentricity > self.config.max_eccentricity {
                diagnostics.rejected_high_eccentricity += 1;
                false
            } else if star.is_cosmic_ray(self.config.max_sharpness) {
                diagnostics.rejected_cosmic_rays += 1;
                false
            } else if !star.is_round(self.config.max_roundness) {
                diagnostics.rejected_roundness += 1;
                false
            } else {
                true
            }
        });

        // Sort by flux (brightest first)
        stars.sort_by(|a, b| {
            b.flux
                .partial_cmp(&a.flux)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Filter FWHM outliers - spurious detections often have abnormally large FWHM
        let removed = filter_fwhm_outliers(&mut stars, self.config.max_fwhm_deviation);
        diagnostics.rejected_fwhm_outliers = removed;
        if removed > 0 {
            tracing::debug!("Removed {} stars with abnormally large FWHM", removed);
        }

        // Remove duplicate detections - keep only the brightest star within min_separation pixels
        let removed = remove_duplicate_stars(&mut stars, self.config.duplicate_min_separation);
        diagnostics.rejected_duplicates = removed;
        if removed > 0 {
            tracing::debug!("Removed {} duplicate star detections", removed);
        }

        // Compute final statistics
        diagnostics.final_star_count = stars.len();

        if !stars.is_empty() {
            let mut buf: Vec<f32> = stars.iter().map(|s| s.fwhm).collect();
            diagnostics.median_fwhm = crate::math::median_f32_mut(&mut buf);

            buf.clear();
            buf.extend(stars.iter().map(|s| s.snr));
            diagnostics.median_snr = crate::math::median_f32_mut(&mut buf);
        }

        StarDetectionResult { stars, diagnostics }
    }
}

/// Result of star detection with diagnostics.
#[derive(Debug, Clone)]
pub struct StarDetectionResult {
    /// Detected stars sorted by flux (brightest first).
    pub stars: Vec<Star>,
    /// Diagnostic information from the detection pipeline.
    pub diagnostics: StarDetectionDiagnostics,
}

/// Diagnostic information from star detection.
///
/// Contains statistics and counts from each stage of the detection pipeline
/// for debugging and tuning purposes.
#[derive(Debug, Clone, Default)]
pub struct StarDetectionDiagnostics {
    /// Number of pixels above detection threshold.
    pub pixels_above_threshold: usize,
    /// Number of connected components found.
    pub connected_components: usize,
    /// Number of candidates after size/edge filtering.
    pub candidates_after_filtering: usize,
    /// Number of candidates that were deblended into multiple stars.
    pub deblended_components: usize,
    /// Number of stars after centroid computation (before quality filtering).
    pub stars_after_centroid: usize,
    /// Number of stars rejected for low SNR.
    pub rejected_low_snr: usize,
    /// Number of stars rejected for high eccentricity.
    pub rejected_high_eccentricity: usize,
    /// Number of stars rejected as cosmic rays (high sharpness).
    pub rejected_cosmic_rays: usize,
    /// Number of stars rejected as saturated.
    pub rejected_saturated: usize,
    /// Number of stars rejected for non-circular shape (roundness).
    pub rejected_roundness: usize,
    /// Number of stars rejected for abnormal FWHM.
    pub rejected_fwhm_outliers: usize,
    /// Number of duplicate detections removed.
    pub rejected_duplicates: usize,
    /// Final number of stars returned.
    pub final_star_count: usize,
    /// Median FWHM of detected stars (pixels).
    pub median_fwhm: f32,
    /// Median SNR of detected stars.
    pub median_snr: f32,
}

/// Remove duplicate star detections that are too close together.
///
/// Keeps the brightest star (by flux) within `min_separation` pixels of each other.
/// Stars must be sorted by flux (brightest first) before calling.
/// Returns the number of stars removed.
fn remove_duplicate_stars(stars: &mut Vec<Star>, min_separation: f32) -> usize {
    if stars.len() < 2 {
        return 0;
    }

    let min_sep_sq = min_separation * min_separation;
    let mut kept = vec![true; stars.len()];

    for i in 0..stars.len() {
        if !kept[i] {
            continue;
        }
        for j in (i + 1)..stars.len() {
            if !kept[j] {
                continue;
            }
            let dx = stars[i].x - stars[j].x;
            let dy = stars[i].y - stars[j].y;
            if dx * dx + dy * dy < min_sep_sq {
                // Keep i (higher flux since sorted), mark j for removal
                kept[j] = false;
            }
        }
    }

    let removed_count = kept.iter().filter(|&&k| !k).count();

    // Filter in place
    let mut write_idx = 0;
    for read_idx in 0..stars.len() {
        if kept[read_idx] {
            if write_idx != read_idx {
                stars[write_idx] = stars[read_idx];
            }
            write_idx += 1;
        }
    }
    stars.truncate(write_idx);

    removed_count
}

/// Compute median and MAD (median absolute deviation) for FWHM filtering.
///
/// Returns (median, mad) computed from the given FWHM values.
fn compute_fwhm_median_mad(fwhms: Vec<f32>) -> (f32, f32) {
    assert!(!fwhms.is_empty(), "Need at least one FWHM value");

    let mut sorted: Vec<f32> = fwhms;
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = sorted[sorted.len() / 2];

    let mut deviations: Vec<f32> = sorted.iter().map(|&f| (f - median).abs()).collect();
    deviations.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mad = deviations[deviations.len() / 2];

    (median, mad)
}

/// Filter stars by FWHM using MAD-based outlier detection.
///
/// Removes stars with FWHM > median + max_deviation * effective_mad.
/// The effective_mad is max(mad, median * 0.1) to handle uniform FWHM.
///
/// Stars should be sorted by flux (brightest first) before calling.
/// Returns the number of stars removed.
fn filter_fwhm_outliers(stars: &mut Vec<Star>, max_deviation: f32) -> usize {
    if max_deviation <= 0.0 || stars.len() < 5 {
        return 0;
    }

    // Use top half for robust median/MAD estimate
    let reference_count = (stars.len() / 2).max(5).min(stars.len());
    let fwhms: Vec<f32> = stars.iter().take(reference_count).map(|s| s.fwhm).collect();
    let (median_fwhm, mad) = compute_fwhm_median_mad(fwhms);

    // Use at least 10% of median as minimum MAD
    let effective_mad = mad.max(median_fwhm * 0.1);
    let max_fwhm = median_fwhm + max_deviation * effective_mad;

    let before_count = stars.len();
    stars.retain(|s| s.fwhm <= max_fwhm);
    before_count - stars.len()
}
