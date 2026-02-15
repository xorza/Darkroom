//! Star detector implementation and related types.
//!
//! This module contains the main [`StarDetector`] struct and its associated
//! types for detecting stars in astronomical images.

pub(crate) mod stages;

#[cfg(test)]
mod bench;

// =============================================================================
// Imports
// =============================================================================

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::astro_image::AstroImage;
use crate::astro_image::error::ImageError;

use super::buffer_pool::BufferPool;
use super::config::Config;
use super::star::Star;

/// Per-channel robust statistics (median and MAD).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ChannelStats {
    pub median: f32,
    pub mad: f32,
}

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

// =============================================================================
// StarDetector
// =============================================================================

/// Star detector with reusable buffer pool.
#[derive(Debug)]
pub struct StarDetector {
    config: Config,
    /// Buffer pool for reusing allocations across detections.
    buffer_pool: Option<BufferPool>,
}

impl Default for StarDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl StarDetector {
    /// Create a new star detector with default configuration.
    pub fn new() -> Self {
        Self {
            config: Config::default(),
            buffer_pool: None,
        }
    }

    /// Create a star detector from an existing configuration.
    pub fn from_config(config: Config) -> Self {
        Self {
            config,
            buffer_pool: None,
        }
    }

    /// Get reference to the underlying configuration.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Clear the buffer pool, freeing all cached memory.
    pub fn clear_buffer_pool(&mut self) {
        self.buffer_pool = None;
    }

    /// Detect stars in a single image.
    pub fn detect(&mut self, image: &AstroImage) -> DetectionResult {
        self.config.validate();

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
        let mut background =
            stages::background::estimate_background(&grayscale_image, &self.config, pool);

        // Step 2b: Refine background if iterative refinement is enabled
        if self.config.refinement.iterations() > 0 {
            stages::background::refine_background(
                &grayscale_image,
                &mut background,
                &self.config,
                pool,
            );
        }

        // Step 3: Determine effective FWHM (manual > auto-estimate > disabled)
        let fwhm_result =
            stages::fwhm::estimate_fwhm(&grayscale_image, &background, &self.config, pool);

        // Step 4: Detect star candidate regions (with optional matched filter)
        let detect_result = stages::detect::detect(
            &grayscale_image,
            &background,
            fwhm_result.fwhm,
            &self.config,
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
            &self.config,
        );
        diagnostics.stars_after_centroid = stars.len();

        // Release image buffers back to pool
        let pool = self.buffer_pool.as_mut().unwrap();
        background.release_to_pool(pool);
        pool.release_f32(grayscale_image);

        // Step 6: Apply quality filters, sort, and remove duplicates
        let (stars, filter_stats) = stages::filter::filter(stars, &self.config);
        filter_stats.apply_to(&mut diagnostics);

        if filter_stats.fwhm_outliers > 0 {
            tracing::debug!(
                "Removed {} stars with abnormally large FWHM",
                filter_stats.fwhm_outliers
            );
        }
        if filter_stats.duplicates > 0 {
            tracing::debug!(
                "Removed {} duplicate star detections",
                filter_stats.duplicates
            );
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

        DetectionResult { stars, diagnostics }
    }

    /// Load an image from `path`, run detection, and save the result as a sidecar file.
    ///
    /// The sidecar is written to `{path}.detection` in SCN format.
    /// Returns the detection result.
    pub fn detect_file(&mut self, path: impl AsRef<Path>) -> Result<DetectionResult, ImageError> {
        let path = path.as_ref();
        let image = AstroImage::from_file(path)?;
        let result = self.detect(&image);
        crate::star_detection::detection_file::save_detection_result(path, &result).map_err(
            |e| ImageError::Io {
                path: path.to_path_buf(),
                source: e,
            },
        )?;
        Ok(result)
    }
}
