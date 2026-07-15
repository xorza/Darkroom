//! Star detector implementation and related types.
//!
//! This module contains the main [`StarDetector`] struct and its associated
//! types for detecting stars in astronomical images.

pub(crate) mod stages;

#[cfg(test)]
mod bench;

use serde::{Deserialize, Serialize};

use crate::io::astro_image::AstroImage;

use crate::math::statistics::median_f32_mut;
use crate::stacking::star_detection::background::{estimate_background, refine_background};
use crate::stacking::star_detection::buffer_pool::BufferPool;
#[cfg(test)]
use crate::stacking::star_detection::buffer_pool::PoolCounts;
use crate::stacking::star_detection::config::Config;
#[cfg(test)]
use crate::stacking::star_detection::config::DetectionConfig;
use crate::stacking::star_detection::detector::stages::filter::FilterOutcome;
use crate::stacking::star_detection::error::StarDetectionConfigError;
use crate::stacking::star_detection::star::Star;

/// Result of star detection with diagnostics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectionResult {
    /// Detected stars sorted by flux (brightest first).
    pub stars: Vec<Star>,
    /// Diagnostic information from the detection pipeline.
    pub diagnostics: Diagnostics,
}

/// Diagnostic information from star detection.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Diagnostics {
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

/// Star detector with reusable buffer pool.
#[derive(Debug)]
pub struct StarDetector {
    config: Config,
    /// Buffer pool for reusing allocations across detections.
    buffer_pool: Option<BufferPool>,
}

impl Default for StarDetector {
    fn default() -> Self {
        Self::from_config(Config::default()).unwrap()
    }
}

impl StarDetector {
    /// Create a star detector from an existing configuration.
    ///
    /// # Errors
    ///
    /// Returns an error when any configuration parameter is invalid.
    pub fn from_config(config: Config) -> Result<Self, StarDetectionConfigError> {
        config.validate()?;
        Ok(Self {
            config,
            buffer_pool: None,
        })
    }

    /// Get reference to the underlying configuration.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Detect stars in a single image.
    pub fn detect(&mut self, image: &AstroImage) -> DetectionResult {
        let width = image.width();
        let height = image.height();

        // Initialize or reset buffer pool for current dimensions
        let pool = self
            .buffer_pool
            .get_or_insert_with(|| BufferPool::new(width, height));
        pool.reset(width, height);

        // Step 1: Image preparation (grayscale, CFA filter)
        let grayscale_image = stages::prepare::prepare(image, pool);

        // Step 2: Estimate background and noise
        let mut background = estimate_background(&grayscale_image, &self.config.background, pool);

        // Step 2b: Refine background if iterative refinement is enabled
        if self.config.background.refinement.iterations() > 0 {
            refine_background(
                &grayscale_image,
                &mut background,
                &self.config.background,
                self.config.detection.sigma_threshold,
                pool,
            );
        }

        // Step 3: Determine effective FWHM (manual > auto-estimate > disabled)
        let fwhm_result = stages::fwhm::estimate_fwhm(
            &grayscale_image,
            &background,
            &self.config.fwhm,
            &self.config.detection,
            &self.config.measurement,
            &self.config.filter,
            pool,
        );

        // Step 4: Detect star candidate regions (with optional matched filter)
        let detect_result = stages::detect::detect(
            &grayscale_image,
            &background,
            fwhm_result.fwhm,
            &self.config.detection,
            pool,
        );

        let mut diagnostics = Diagnostics {
            pixels_above_threshold: detect_result.pixels_above_threshold,
            connected_components: detect_result.connected_components,
            candidates_after_filtering: detect_result.regions.len(),
            deblended_components: detect_result.deblended_components,
            estimated_fwhm: fwhm_result.fwhm.unwrap_or(0.0),
            fwhm_estimation_star_count: fwhm_result.stars_used,
            fwhm_was_auto_estimated: fwhm_result.stars_used > 0,
            ..Default::default()
        };
        tracing::debug!("Detected {} star candidates", detect_result.regions.len());

        // Step 5: Compute precise centroids (parallel)
        let stars = stages::measure::measure(
            &detect_result.regions,
            &grayscale_image,
            &background,
            &self.config.measurement,
            self.config.fwhm.expected,
        );
        diagnostics.stars_after_centroid = stars.len();

        // Release image buffers back to pool
        let pool = self.buffer_pool.as_mut().unwrap();
        background.release_to_pool(pool);
        pool.release_f32(grayscale_image);

        // Step 6: Apply quality filters, sort, and remove duplicates
        let FilterOutcome { stars, stats } = stages::filter::filter(stars, &self.config.filter);
        stats.apply_to(&mut diagnostics);

        if stats.fwhm_outliers > 0 {
            tracing::debug!(
                "Removed {} stars with abnormally large FWHM",
                stats.fwhm_outliers
            );
        }
        if stats.duplicates > 0 {
            tracing::debug!("Removed {} duplicate star detections", stats.duplicates);
        }

        // Compute final statistics
        diagnostics.final_star_count = stars.len();
        if !stars.is_empty() {
            let mut buf: Vec<f32> = stars.iter().map(|s| s.fwhm).collect();
            diagnostics.median_fwhm = median_f32_mut(&mut buf);

            buf.clear();
            buf.extend(stars.iter().map(|s| s.snr));
            diagnostics.median_snr = median_f32_mut(&mut buf);
        }

        DetectionResult { stars, diagnostics }
    }
}

#[cfg(test)]
impl StarDetector {
    /// Current buffer-pool working-set counts, or `None` before the first `detect`. For the memory
    /// tests that assert the pool stays flat in the frame count across repeated detections.
    pub(crate) fn pool_counts(&self) -> Option<PoolCounts> {
        self.buffer_pool.as_ref().map(|pool| pool.counts())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructor_rejects_invalid_configuration() {
        let error = StarDetector::from_config(Config {
            detection: DetectionConfig {
                sigma_threshold: 0.0,
                ..Default::default()
            },
            ..Config::default()
        })
        .unwrap_err();
        assert_eq!(
            error,
            StarDetectionConfigError::InvalidSigmaThreshold { value: 0.0 }
        );
    }
}
