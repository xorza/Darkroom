//! [`StarDetector`]: a detection's stages run in order, over buffers it keeps between frames.

pub(crate) mod detection_result;
pub(super) mod stages;

use crate::image::linear_image::LinearImage;
use crate::math::size2us::Size2us;

use crate::error::InvalidConfigField;
use crate::math::statistics::median_mut;
use crate::star_detection::resources::DetectionResources;
use crate::star_detection::star_detection_config::StarDetectionConfig;
use crate::star_detection::star_detector::detection_result::{
    DetectionDiagnostics, DetectionResult,
};
use crate::star_detection::star_detector::stages::detect::DetectResult;
use crate::star_detection::star_detector::stages::filter::FilterOutcome;
use crate::star_detection::star_detector::stages::fwhm;
use crate::star_detection::star_detector::stages::prepared_frame::PreparedFrame;

/// Star detector with reusable processing resources.
#[derive(Debug)]
pub struct StarDetector {
    config: StarDetectionConfig,
    /// Working memory retained across detections.
    resources: Option<DetectionResources>,
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
            resources: None,
        })
    }

    /// Detect stars in a single image.
    pub fn detect(&mut self, image: &LinearImage) -> DetectionResult {
        let width = image.width();
        let height = image.height();

        let resources = self
            .resources
            .get_or_insert_with(|| DetectionResources::new(Size2us::new(width, height)));
        resources.reset(Size2us::new(width, height));

        let mut frame = PreparedFrame::new(image, &self.config, resources);
        let fwhm = fwhm::estimate(&frame, &self.config, resources);
        let plane = frame.detection_plane(fwhm.value(), &self.config, resources);
        frame.release_sources(resources);
        let detect_result = DetectResult::from_plane(
            &plane,
            frame.no_data.as_ref(),
            &self.config.extraction,
            resources,
        );
        plane.release_to_pool(resources);

        let mut diagnostics = DetectionDiagnostics {
            pixels_above_threshold: detect_result.pixels_above_threshold,
            connected_components: detect_result.connected_components,
            candidates_after_filtering: detect_result.regions.len(),
            deblended_components: detect_result.deblended_components,
            fwhm,
            ..Default::default()
        };
        tracing::debug!("Detected {} star candidates", detect_result.regions.len());

        let stars = stages::measure::measure(
            &detect_result.regions,
            &frame,
            &self.config.measurement,
            fwhm.value(),
        );
        diagnostics.stars_after_centroid = stars.len();
        frame.release_to_pool(resources);

        let FilterOutcome {
            stars,
            diagnostics: quality_filter,
        } = FilterOutcome::from_stars(
            stars,
            &self.config.filter,
            &mut resources.values,
            &mut resources.duplicates,
        );
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
            diagnostics.median_fwhm = Some(median_mut(values));
            values.clear();
            values.extend(stars.iter().map(|s| s.snr));
            diagnostics.median_snr = Some(median_mut(values));
        }

        DetectionResult { stars, diagnostics }
    }
}

#[cfg(test)]
pub(super) mod internals {
    use crate::star_detection::resources::internals::BufferCounts;
    use crate::star_detection::resources::internals::buffer_counts;
    use crate::star_detection::star_detector::StarDetector;

    pub(crate) fn buffer_counts_for(detector: &StarDetector) -> Option<BufferCounts> {
        detector.resources.as_ref().map(buffer_counts)
    }
}

#[cfg(feature = "bench")]
pub(crate) mod bench;

#[cfg(test)]
mod tests;
