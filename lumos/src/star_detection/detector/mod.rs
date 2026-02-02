//! Star detector implementation and related types.
//!
//! This module contains the main [`StarDetector`] struct and its associated
//! types for detecting stars in astronomical images.

#[cfg(test)]
mod bench;
#[cfg(test)]
mod tests;

// =============================================================================
// Imports
// =============================================================================

use crate::astro_image::AstroImage;
use crate::common::Buffer2;

use super::background::{BackgroundConfig, BackgroundMap};
use super::candidate_detection::{self, detect_stars};
use super::centroid::compute_centroid;
use super::config::StarDetectionConfig;
use super::convolution::matched_filter;
use super::fwhm_estimation;
use super::median_filter::median_filter_3x3;
use super::star::Star;

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
    /// Estimated FWHM from auto-estimation (0.0 if not used or disabled).
    pub estimated_fwhm: f32,
    /// Number of stars used for FWHM estimation.
    pub fwhm_estimation_star_count: usize,
    /// Whether FWHM was auto-estimated (true) or manual/disabled (false).
    pub fwhm_was_auto_estimated: bool,
}

// =============================================================================
// StarDetector
// =============================================================================

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
/// let config = StarDetectionConfig {
///     psf: PsfConfig {
///         expected_fwhm: 4.0,
///         ..Default::default()
///     },
///     filtering: FilteringConfig {
///         min_snr: 15.0,
///         edge_margin: 20,
///         ..Default::default()
///     },
///     ..Default::default()
/// };
/// let detector = StarDetector::from_config(config);
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
        let mut grayscale_image = image.to_grayscale_buffer();
        let mut scratch = Buffer2::new_default(width, height);

        // Step 0a: Apply defect mask if provided
        if let Some(defect_map) = self.config.defect_map.as_ref()
            && !defect_map.is_empty()
        {
            defect_map.apply(&grayscale_image, &mut scratch);
            std::mem::swap(&mut grayscale_image, &mut scratch);
        }

        // Step 0b: Apply 3x3 median filter to remove Bayer pattern artifacts
        // Only applied for CFA sensors; skip for monochrome (~6ms faster on 4K images)
        if image.metadata.is_cfa {
            median_filter_3x3(&grayscale_image, &mut scratch);
            std::mem::swap(&mut grayscale_image, &mut scratch);
        }

        // Step 1: Estimate background
        let background = BackgroundMap::new(&grayscale_image, &self.config.background);

        // Step 2: Determine effective FWHM (manual > auto-estimate > disabled)
        let (effective_fwhm, fwhm_estimate) =
            self.determine_effective_fwhm(&grayscale_image, &background);

        // Step 3: Detect star candidates (with optional matched filter)
        // Reuse scratch buffer for matched filter output
        let candidates =
            self.detect_candidates(&grayscale_image, &background, effective_fwhm, &mut scratch);

        let mut diagnostics = StarDetectionDiagnostics {
            candidates_after_filtering: candidates.len(),
            estimated_fwhm: fwhm_estimate.as_ref().map_or(0.0, |e| e.fwhm),
            fwhm_estimation_star_count: fwhm_estimate.as_ref().map_or(0, |e| e.star_count),
            fwhm_was_auto_estimated: fwhm_estimate.as_ref().is_some_and(|e| e.is_estimated),
            ..Default::default()
        };
        tracing::debug!("Detected {} star candidates", candidates.len());

        // Step 4: Compute precise centroids (parallel)
        let mut stars = compute_centroids(candidates, &grayscale_image, &background, &self.config);
        diagnostics.stars_after_centroid = stars.len();

        // Step 5: Apply quality filters
        let filter_stats = apply_quality_filters(&mut stars, &self.config);
        diagnostics.rejected_saturated = filter_stats.saturated;
        diagnostics.rejected_low_snr = filter_stats.low_snr;
        diagnostics.rejected_high_eccentricity = filter_stats.high_eccentricity;
        diagnostics.rejected_cosmic_rays = filter_stats.cosmic_rays;
        diagnostics.rejected_roundness = filter_stats.roundness;

        // Step 6: Post-processing
        sort_by_flux(&mut stars);

        let removed = filter_fwhm_outliers(&mut stars, self.config.filtering.max_fwhm_deviation);
        diagnostics.rejected_fwhm_outliers = removed;
        if removed > 0 {
            tracing::debug!("Removed {} stars with abnormally large FWHM", removed);
        }

        let removed =
            remove_duplicate_stars(&mut stars, self.config.filtering.duplicate_min_separation);
        diagnostics.rejected_duplicates = removed;
        if removed > 0 {
            tracing::debug!("Removed {} duplicate star detections", removed);
        }

        // Step 7: Compute final statistics
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

    /// Determine effective FWHM for matched filtering.
    ///
    /// Priority: manual expected_fwhm > auto-estimate > disabled (0.0)
    fn determine_effective_fwhm(
        &self,
        pixels: &Buffer2<f32>,
        background: &BackgroundMap,
    ) -> (f32, Option<fwhm_estimation::FwhmEstimate>) {
        if self.config.psf.expected_fwhm > f32::EPSILON {
            return (self.config.psf.expected_fwhm, None);
        }

        if self.config.psf.auto_estimate {
            let estimate = self.estimate_fwhm_from_bright_stars(pixels, background);
            return (estimate.fwhm, Some(estimate));
        }

        (0.0, None)
    }

    /// Detect star candidates, optionally applying matched filter.
    fn detect_candidates(
        &self,
        pixels: &Buffer2<f32>,
        background: &BackgroundMap,
        effective_fwhm: f32,
        scratch: &mut Buffer2<f32>,
    ) -> Vec<candidate_detection::StarCandidate> {
        let filtered: Option<&Buffer2<f32>> = if effective_fwhm > f32::EPSILON {
            tracing::debug!(
                "Applying matched filter with FWHM={:.1}, axis_ratio={:.2}, angle={:.1}°",
                effective_fwhm,
                self.config.psf.axis_ratio,
                self.config.psf.angle.to_degrees()
            );
            matched_filter(
                pixels,
                &background.background,
                effective_fwhm,
                self.config.psf.axis_ratio,
                self.config.psf.angle,
                scratch,
            );
            Some(scratch)
        } else {
            None
        };

        detect_stars(pixels, filtered, background, &self.config)
    }

    /// Perform first-pass detection and estimate FWHM from bright stars.
    fn estimate_fwhm_from_bright_stars(
        &self,
        pixels: &Buffer2<f32>,
        background: &BackgroundMap,
    ) -> fwhm_estimation::FwhmEstimate {
        // Create a modified config for first-pass detection:
        // - Higher sigma threshold: detect only the brightest stars for reliable FWHM measurement
        // - No matched filter (expected_fwhm=0): we don't know the FWHM yet
        // - Smaller min_area: more permissive to catch small bright stars
        // - Higher min_snr: ensure high-quality stars for accurate estimation
        let first_pass_config = StarDetectionConfig {
            background: BackgroundConfig {
                sigma_threshold: self.config.background.sigma_threshold
                    * self.config.psf.estimation_sigma_factor,
                ..self.config.background.clone()
            },
            psf: super::config::PsfConfig {
                expected_fwhm: 0.0,
                ..self.config.psf
            },
            filtering: super::config::FilteringConfig {
                min_area: 3,
                min_snr: self.config.filtering.min_snr * 2.0,
                ..self.config.filtering
            },
            ..self.config.clone()
        };

        let candidates = detect_stars(pixels, None, background, &first_pass_config);
        tracing::debug!(
            "FWHM estimation: first pass detected {} bright star candidates",
            candidates.len()
        );

        let stars = compute_centroids(candidates, pixels, background, &first_pass_config);

        fwhm_estimation::estimate_fwhm(
            &stars,
            self.config.psf.min_stars_for_estimation,
            4.0,
            self.config.filtering.max_eccentricity,
            self.config.filtering.max_sharpness,
        )
    }
}

// =============================================================================
// Helper Functions
// =============================================================================

/// Compute centroids for star candidates in parallel.
fn compute_centroids(
    candidates: Vec<candidate_detection::StarCandidate>,
    pixels: &Buffer2<f32>,
    background: &BackgroundMap,
    config: &StarDetectionConfig,
) -> Vec<Star> {
    use rayon::prelude::*;

    candidates
        .into_par_iter()
        .filter_map(|candidate| compute_centroid(pixels, background, &candidate, config))
        .collect()
}

/// Apply quality filters to stars, returning rejection counts.
fn apply_quality_filters(
    stars: &mut Vec<Star>,
    config: &StarDetectionConfig,
) -> QualityFilterStats {
    let mut stats = QualityFilterStats::default();

    stars.retain(|star| {
        if star.is_saturated() {
            stats.saturated += 1;
            false
        } else if star.snr < config.filtering.min_snr {
            stats.low_snr += 1;
            false
        } else if star.eccentricity > config.filtering.max_eccentricity {
            stats.high_eccentricity += 1;
            false
        } else if star.is_cosmic_ray(config.filtering.max_sharpness) {
            stats.cosmic_rays += 1;
            false
        } else if !star.is_round(config.filtering.max_roundness) {
            stats.roundness += 1;
            false
        } else {
            true
        }
    });

    stats
}

#[derive(Debug, Default)]
struct QualityFilterStats {
    saturated: usize,
    low_snr: usize,
    high_eccentricity: usize,
    cosmic_rays: usize,
    roundness: usize,
}

/// Sort stars by flux (brightest first).
fn sort_by_flux(stars: &mut [Star]) {
    stars.sort_by(|a, b| {
        b.flux
            .partial_cmp(&a.flux)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

/// Remove duplicate star detections that are too close together.
///
/// Keeps the brightest star (by flux) within `min_separation` pixels.
/// Stars must be sorted by flux (brightest first) before calling.
pub(crate) fn remove_duplicate_stars(stars: &mut Vec<Star>, min_separation: f32) -> usize {
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
                kept[j] = false;
            }
        }
    }

    let removed_count = kept.iter().filter(|&&k| !k).count();

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

/// Filter stars by FWHM using MAD-based outlier detection.
///
/// Removes stars with FWHM > median + max_deviation × effective_mad.
/// Stars should be sorted by flux (brightest first) before calling.
pub(crate) fn filter_fwhm_outliers(stars: &mut Vec<Star>, max_deviation: f32) -> usize {
    if max_deviation <= 0.0 || stars.len() < 5 {
        return 0;
    }

    let reference_count = (stars.len() / 2).max(5).min(stars.len());
    let mut fwhms: Vec<f32> = stars.iter().take(reference_count).map(|s| s.fwhm).collect();
    let (median_fwhm, mad) = crate::math::median_and_mad_f32_mut(&mut fwhms);

    let effective_mad = mad.max(median_fwhm * 0.1);
    let max_fwhm = median_fwhm + max_deviation * effective_mad;

    let before_count = stars.len();
    stars.retain(|s| s.fwhm <= max_fwhm);
    before_count - stars.len()
}
