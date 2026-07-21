//! Star detector implementation and related types.
//!
//! This module contains the main [`StarDetector`] struct and its associated
//! types for detecting stars in astronomical images.

pub(crate) mod stages;

#[cfg(test)]
mod bench;

use serde::{Deserialize, Serialize};

use crate::io::image::linear::LinearImage;

use crate::math::statistics::median_f32_mut;
use crate::stacking::star_detection::background::{estimate_background, refine_background};
use crate::stacking::star_detection::config::Config;
#[cfg(test)]
use crate::stacking::star_detection::config::DetectionConfig;
use crate::stacking::star_detection::detector::stages::filter::FilterOutcome;
use crate::stacking::star_detection::error::StarDetectionConfigError;
use crate::stacking::star_detection::resources::DetectionResources;
use crate::stacking::star_detection::star::Star;

/// Result of star detection with diagnostics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectionResult {
    /// Detected stars sorted by flux (brightest first).
    pub stars: Vec<Star>,
    /// Diagnostic information from the detection pipeline.
    pub diagnostics: Diagnostics,
}

/// Rejection counts produced by the quality-filtering stage.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QualityFilterDiagnostics {
    /// Number of stars rejected as saturated.
    pub saturated: usize,
    /// Number of stars rejected for low SNR.
    pub low_snr: usize,
    /// Number of stars rejected for high eccentricity.
    pub high_eccentricity: usize,
    /// Number of stars rejected as cosmic rays.
    pub cosmic_rays: usize,
    /// Number of stars rejected for non-circular shape.
    pub roundness: usize,
    /// Number of stars rejected for abnormal FWHM.
    pub fwhm_outliers: usize,
    /// Number of duplicate detections removed.
    pub duplicates: usize,
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
    /// Rejections produced by the quality-filtering stage.
    pub quality_filter: QualityFilterDiagnostics,
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

/// Star detector with reusable processing resources.
#[derive(Debug)]
pub struct StarDetector {
    config: Config,
    /// Working memory retained across detections.
    resources: Option<DetectionResources>,
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
            resources: None,
        })
    }

    /// Get reference to the underlying configuration.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Detect stars in a single image.
    pub fn detect(&mut self, image: &LinearImage) -> DetectionResult {
        let width = image.width();
        let height = image.height();

        let resources = self
            .resources
            .get_or_insert_with(|| DetectionResources::new(width, height));
        resources.reset(width, height);

        // Step 1: Image preparation (grayscale, CFA filter)
        let grayscale_image = stages::prepare::prepare(image, resources);

        // Step 2: Estimate background and noise
        let mut background =
            estimate_background(&grayscale_image, &self.config.background, resources);

        // Step 2b: Refine background if iterative refinement is enabled
        if self.config.background.refinement.iterations() > 0 {
            refine_background(
                &grayscale_image,
                &mut background,
                &self.config.background,
                self.config.detection.sigma_threshold,
                resources,
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
            resources,
        );
        let effective_fwhm = fwhm_result.fwhm.unwrap_or(0.0);

        // Step 4: Detect star candidate regions (with optional matched filter)
        let detect_result = stages::detect::detect(
            &grayscale_image,
            &background,
            fwhm_result.fwhm,
            &self.config.detection,
            resources,
        );

        let mut diagnostics = Diagnostics {
            pixels_above_threshold: detect_result.pixels_above_threshold,
            connected_components: detect_result.connected_components,
            candidates_after_filtering: detect_result.regions.len(),
            deblended_components: detect_result.deblended_components,
            estimated_fwhm: effective_fwhm,
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
            effective_fwhm,
        );
        diagnostics.stars_after_centroid = stars.len();

        background.release_to_pool(resources);
        resources.release_f32(grayscale_image);

        // Step 6: Apply quality filters, sort, and remove duplicates
        let FilterOutcome {
            stars,
            diagnostics: quality_filter,
        } = stages::filter::filter(stars, &self.config.filter);
        diagnostics.quality_filter = quality_filter;

        if diagnostics.quality_filter.fwhm_outliers > 0 {
            tracing::debug!(
                "Removed {} stars with abnormally large FWHM",
                diagnostics.quality_filter.fwhm_outliers
            );
        }
        if diagnostics.quality_filter.duplicates > 0 {
            tracing::debug!(
                "Removed {} duplicate star detections",
                diagnostics.quality_filter.duplicates
            );
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
pub(crate) mod test_support {
    use crate::stacking::star_detection::detector::StarDetector;
    use crate::stacking::star_detection::resources::test_support::BufferCounts;
    use crate::stacking::star_detection::resources::test_support::buffer_counts;

    pub(crate) fn buffer_counts_for(detector: &StarDetector) -> Option<BufferCounts> {
        detector.resources.as_ref().map(buffer_counts)
    }
}

#[cfg(test)]
mod tests {
    use crate::stacking::star_detection::detector::*;
    use crate::stacking::star_detection::synthetic_tests::Scenario;

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

    #[test]
    fn auto_estimated_fwhm_is_used_for_final_measurement() {
        for (actual_fwhm, configured_seed, flux) in
            [(2.5, 8.0, (3.0, 8.0)), (7.0, 1.0, (10.0, 30.0))]
        {
            let frame = Scenario {
                num_stars: 40,
                flux,
                fwhm: actual_fwhm,
                ..Default::default()
            }
            .frame();
            let mut auto_config = Config::default();
            auto_config.fwhm.expected = configured_seed;
            auto_config.fwhm.auto_estimate = true;
            auto_config.fwhm.min_stars = 5;
            auto_config.filter.min_snr = 1.0;
            auto_config.filter.max_eccentricity = 1.0;
            auto_config.filter.max_sharpness = 1.0;
            auto_config.filter.max_roundness = 1.0;
            auto_config.filter.max_fwhm_deviation = 0.0;
            auto_config.filter.duplicate_min_separation = 0.0;

            let auto_result = StarDetector::from_config(auto_config.clone())
                .unwrap()
                .detect(&frame.image);
            assert!(
                auto_result.diagnostics.fwhm_was_auto_estimated,
                "FWHM {actual_fwhm} fixture must produce a genuine estimate"
            );
            let effective_fwhm = auto_result.diagnostics.estimated_fwhm;
            assert!(
                (effective_fwhm - configured_seed).abs() > 1.0,
                "fixture must estimate far from its configured seed: estimate {effective_fwhm}, seed {configured_seed}"
            );

            let mut manual_config = auto_config;
            manual_config.fwhm.expected = effective_fwhm;
            manual_config.fwhm.auto_estimate = false;
            let manual_result = StarDetector::from_config(manual_config)
                .unwrap()
                .detect(&frame.image);

            assert_eq!(
                auto_result.stars.len(),
                manual_result.stars.len(),
                "auto and equivalent manual FWHM must retain the same stars for PSF {actual_fwhm}"
            );
            for (auto, manual) in auto_result.stars.iter().zip(&manual_result.stars) {
                assert_eq!(auto.pos, manual.pos);
                assert_eq!(auto.flux.to_bits(), manual.flux.to_bits());
                assert_eq!(auto.fwhm.to_bits(), manual.fwhm.to_bits());
                assert_eq!(auto.eccentricity.to_bits(), manual.eccentricity.to_bits());
                assert_eq!(auto.snr.to_bits(), manual.snr.to_bits());
                assert_eq!(auto.peak.to_bits(), manual.peak.to_bits());
                assert_eq!(auto.sharpness.to_bits(), manual.sharpness.to_bits());
                assert_eq!(auto.roundness1.to_bits(), manual.roundness1.to_bits());
                assert_eq!(auto.roundness2.to_bits(), manual.roundness2.to_bits());
            }
        }
    }
}
