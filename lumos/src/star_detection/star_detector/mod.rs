//! [`StarDetector`]: a detection's stages run in order, over buffers it keeps between frames.

pub(crate) mod detection_result;
pub(super) mod stages;

use crate::image::linear_image::LinearImage;
use crate::math::size2us::Size2us;

use crate::error::InvalidConfigField;
use crate::math::statistics;
use crate::star_detection::resources::DetectionResources;
use crate::star_detection::star_detection_config::StarDetectionConfig;
use crate::star_detection::star_detector::detection_result::{
    DetectionDiagnostics, DetectionResult,
};
use crate::star_detection::star_detector::stages::detect::{Candidates, DetectionInput};
use crate::star_detection::star_detector::stages::filter::FilterOutcome;
use crate::star_detection::star_detector::stages::prepared_frame::{MeasureRequest, PreparedFrame};

/// Star detector with reusable processing resources.
#[derive(Debug)]
pub struct StarDetector {
    config: StarDetectionConfig,
    /// Working memory retained across detections.
    resources: DetectionResources,
}

impl Default for StarDetector {
    fn default() -> Self {
        Self::from_config(StarDetectionConfig::default()).unwrap()
    }
}

impl StarDetector {
    /// Create a star detector from an existing configuration.
    ///
    /// # Errors
    ///
    /// Returns an error when any configuration parameter is invalid.
    pub fn from_config(config: StarDetectionConfig) -> Result<Self, InvalidConfigField> {
        config.validate()?;
        Ok(Self {
            config,
            resources: DetectionResources::default(),
        })
    }

    /// Detect stars in a single image.
    pub fn detect(&mut self, image: &LinearImage) -> DetectionResult {
        let config = &self.config;
        let resources = &mut self.resources;
        resources.reshape(Size2us::new(image.width(), image.height()));

        let mut frame = PreparedFrame::new(image, config, resources);
        let fwhm = frame.estimate_fwhm(config, resources);
        let plane = frame.final_plane(fwhm.value(), config, resources);
        let candidates = Candidates::detect(
            DetectionInput {
                plane: &plane,
                no_data: frame.no_data.as_deref(),
            },
            &config.extraction,
            resources,
        );
        drop(plane);
        tracing::debug!("Detected {} star candidates", candidates.regions.len());

        let mut stars = Vec::new();
        frame.measure_stars(
            MeasureRequest {
                regions: &candidates.regions,
                config: &config.measurement,
                expected_fwhm: fwhm.value(),
            },
            &mut resources.measured,
            &mut stars,
        );
        let mut diagnostics = DetectionDiagnostics {
            pixels_above_threshold: candidates.pixels_above_threshold,
            connected_components: candidates.connected_components,
            candidates_after_filtering: candidates.regions.len(),
            deblended_components: candidates.deblended_components,
            stars_after_centroid: stars.len(),
            fwhm,
            ..Default::default()
        };
        candidates.release(resources);

        let FilterOutcome {
            stars,
            diagnostics: quality_filter,
        } = FilterOutcome::from_stars(stars, &config.filter, resources);
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

        if !stars.is_empty() {
            let values = &mut resources.values;
            values.clear();
            values.extend(stars.iter().map(|s| s.fwhm));
            diagnostics.median_fwhm = Some(statistics::median_mut(values));
            values.clear();
            values.extend(stars.iter().map(|s| s.snr));
            diagnostics.median_snr = Some(statistics::median_mut(values));
        }

        DetectionResult { stars, diagnostics }
    }
}

#[cfg(test)]
pub(super) mod internals {
    use crate::star_detection::resources::internals::BufferCounts;
    use crate::star_detection::star_detector::StarDetector;

    impl StarDetector {
        pub(crate) fn buffer_counts(&self) -> BufferCounts {
            self.resources.buffer_counts()
        }
    }
}

#[cfg(feature = "internals")]
pub(crate) mod bench;

#[cfg(test)]
mod tests;
