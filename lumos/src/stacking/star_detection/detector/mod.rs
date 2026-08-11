//! Star detector implementation and related types.
//!
//! This module contains the main [`StarDetector`] struct and its associated
//! types for detecting stars in astronomical images.

pub(super) mod stages;

#[cfg(all(test, feature = "internals"))]
mod bench;

use serde::{Deserialize, Serialize};

use crate::io::image::linear::LinearImage;
use crate::math::size2us::Size2us;

use crate::error::InvalidConfigField;
use crate::math::statistics::median_f32_mut;
use crate::stacking::star_detection::background::background_estimate::BackgroundEstimate;
use crate::stacking::star_detection::config::Config;
use crate::stacking::star_detection::detector::stages::detect::DetectResult;
use crate::stacking::star_detection::detector::stages::filter::FilterOutcome;
use crate::stacking::star_detection::detector::stages::fwhm;
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
    /// Where the matched filter's FWHM came from.
    pub fwhm: FwhmSource,
}

/// Where the FWHM the detector ran with came from — the three states the matched-filter stage can
/// end in, as one value rather than an `f32` plus a count plus a flag derived from the count, which
/// admits combinations none of the three states describes.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub enum FwhmSource {
    /// Matched filtering was off: auto-estimation disabled and no configured FWHM.
    #[default]
    Disabled,
    /// Taken from configuration, or from the built-in fallback when too few stars passed to
    /// estimate one. Nothing was measured from this frame.
    Configured(f32),
    /// Measured from this frame's own stars.
    Estimated {
        fwhm: f32,
        /// Stars that contributed to the estimate; always non-zero.
        stars_used: usize,
    },
}

impl FwhmSource {
    /// The FWHM the detector ran with, or `None` when matched filtering was off.
    pub fn value(&self) -> Option<f32> {
        match self {
            FwhmSource::Disabled => None,
            FwhmSource::Configured(fwhm) => Some(*fwhm),
            FwhmSource::Estimated { fwhm, .. } => Some(*fwhm),
        }
    }

    /// Whether the FWHM was measured from this frame rather than supplied.
    pub fn was_estimated(&self) -> bool {
        matches!(self, FwhmSource::Estimated { .. })
    }
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
    pub fn from_config(config: Config) -> Result<Self, InvalidConfigField> {
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
            .get_or_insert_with(|| DetectionResources::new(Size2us::new(width, height)));
        resources.reset(Size2us::new(width, height));

        // Step 1: Image preparation (grayscale, CFA filter)
        let grayscale_image = stages::prepare::prepare(image, resources);

        // Step 2: Estimate background and noise
        let mut background =
            BackgroundEstimate::estimate(&grayscale_image, &self.config.background, resources);

        // Step 2b: Refine background if iterative refinement is enabled
        if self.config.background.refinement.iterations() > 0 {
            background.refine(
                &grayscale_image,
                &self.config.background,
                self.config.detection.sigma_threshold,
                resources,
            );
        }

        // Step 3: Determine effective FWHM (manual > auto-estimate > disabled)
        let fwhm = fwhm::estimate(&grayscale_image, &background, &self.config, resources);
        let effective_fwhm = fwhm.value().unwrap_or(0.0);

        // Step 4: Detect star candidate regions (with optional matched filter)
        let detect_result = DetectResult::from_image(
            &grayscale_image,
            &background,
            fwhm.value(),
            &self.config.detection,
            resources,
        );

        let mut diagnostics = Diagnostics {
            pixels_above_threshold: detect_result.pixels_above_threshold,
            connected_components: detect_result.connected_components,
            candidates_after_filtering: detect_result.regions.len(),
            deblended_components: detect_result.deblended_components,
            fwhm,
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
        } = FilterOutcome::from_stars(stars, &self.config.filter);
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
pub(super) mod internals {
    use crate::stacking::star_detection::detector::StarDetector;
    use crate::stacking::star_detection::resources::internals::BufferCounts;
    use crate::stacking::star_detection::resources::internals::buffer_counts;

    pub(crate) fn buffer_counts_for(detector: &StarDetector) -> Option<BufferCounts> {
        detector.resources.as_ref().map(buffer_counts)
    }
}

#[cfg(test)]
mod tests {
    use crate::stacking::star_detection::config::detection_config::DetectionConfig;
    use crate::stacking::star_detection::detector::*;
    use crate::stacking::star_detection::tests::Scenario;

    #[test]
    fn fwhm_source_distinguishes_measured_from_supplied() {
        // `value` reports what the detector ran with; only a measured one is `was_estimated`.
        assert_eq!(FwhmSource::Configured(3.5).value(), Some(3.5));
        assert_eq!(
            FwhmSource::Estimated {
                fwhm: 3.5,
                stars_used: 12
            }
            .value(),
            Some(3.5)
        );
        assert_eq!(FwhmSource::Disabled.value(), None);
        assert!(!FwhmSource::Configured(3.5).was_estimated());
        assert!(
            FwhmSource::Estimated {
                fwhm: 3.5,
                stars_used: 12
            }
            .was_estimated()
        );
        assert!(!FwhmSource::Disabled.was_estimated());
        // A default-constructed `Diagnostics` reports no FWHM rather than a bogus 0.0.
        assert_eq!(Diagnostics::default().fwhm, FwhmSource::Disabled);
    }

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
        assert_eq!((error.field, error.value), ("sigma_threshold", 0.0));
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
                auto_result.diagnostics.fwhm.was_estimated(),
                "FWHM {actual_fwhm} fixture must produce a genuine estimate"
            );
            let effective_fwhm = auto_result
                .diagnostics
                .fwhm
                .value()
                .expect("an estimated FWHM has a value");
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
                assert_eq!(
                    auto.roundness.ground.to_bits(),
                    manual.roundness.ground.to_bits()
                );
                assert_eq!(
                    auto.roundness.sround.to_bits(),
                    manual.roundness.sround.to_bits()
                );
            }
        }
    }
}
