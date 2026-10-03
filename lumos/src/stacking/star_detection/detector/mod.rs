//! Star detector implementation and related types.
//!
//! This module contains the main [`StarDetector`] struct and its associated
//! types for detecting stars in astronomical images.

pub(super) mod stages;

use serde::{Deserialize, Serialize};

use imaginarium::Buffer2;

use crate::bit_buffer2::BitBuffer2;
use crate::io::image::linear::LinearImage;
use crate::math::size2us::Size2us;

use crate::error::InvalidConfigField;
use crate::math::statistics::median_mut;
use crate::stacking::star_detection::background::background_estimate::{
    BackgroundEstimate, Refinement,
};
use crate::stacking::star_detection::config::Config;
use crate::stacking::star_detection::config::background_config::BackgroundRefinement;
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
    /// Median FWHM of the detected stars in pixels, `None` when none was detected.
    pub median_fwhm: Option<f32>,
    /// Median SNR of the detected stars, `None` when none was detected.
    pub median_snr: Option<f32>,
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
    pub const fn value(&self) -> Option<f32> {
        match self {
            FwhmSource::Disabled => None,
            FwhmSource::Configured(fwhm) => Some(*fwhm),
            FwhmSource::Estimated { fwhm, .. } => Some(*fwhm),
        }
    }
}

/// A pixel at this fraction of the data's ceiling counts as saturated: a clipped star's flat top
/// rarely reads the ceiling itself once dark subtraction and the decoder's scaling have moved it.
const SATURATION_FRACTION: f32 = 0.95;

/// The level a pixel of `image` saturates at: [`SATURATION_FRACTION`] of its declared ceiling,
/// `DATAMAX` in the normalized domain, or of that domain's 1 when it declares none.
fn saturation_level(image: &LinearImage) -> f32 {
    SATURATION_FRACTION * image.metadata.data_max.map_or(1.0, |max| max as f32)
}

/// Mark the pixels of `values` at or above `level`.
fn mark_saturated(values: &Buffer2<f32>, level: f32, mask: &mut BitBuffer2) {
    let values = values.pixels();
    mask.fill_from_predicate(|index| values[index] >= level);
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

    /// Detect stars in a single image.
    pub fn detect(&mut self, image: &LinearImage) -> DetectionResult {
        let width = image.width();
        let height = image.height();

        let resources = self
            .resources
            .get_or_insert_with(|| DetectionResources::new(Size2us::new(width, height)));
        resources.reset(Size2us::new(width, height));

        let mut residual = stages::prepare::prepare(image, resources);

        let mut background =
            BackgroundEstimate::estimate(&residual, &self.config.background, resources);
        if let BackgroundRefinement::Iterative {
            iterations,
            mask_dilation,
        } = self.config.background.refinement
        {
            background.refine(
                &residual,
                &self.config.background,
                Refinement {
                    iterations,
                    mask_dilation,
                },
                self.config.detection.sigma_threshold,
                resources,
            );
        }

        // Saturation is a property of the recorded values, which the subtraction below removes.
        let mut saturation = resources.acquire_bit();
        mark_saturated(&residual, saturation_level(image), &mut saturation);

        // From here on every stage reads the residual: no threshold, deblend or measurement sees
        // the sky.
        let sky = background.subtract_from(&mut residual, resources);

        let fwhm = fwhm::estimate(&residual, &sky, &saturation, &self.config, resources);
        let detect_result = DetectResult::from_image(
            &residual,
            &sky,
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

        let stars = stages::measure::measure(
            &detect_result.regions,
            &residual,
            &sky,
            &saturation,
            &self.config.measurement,
            fwhm.value(),
        );
        diagnostics.stars_after_centroid = stars.len();

        resources.release_bit(saturation);
        sky.release_to_pool(resources);
        resources.release_f32(residual);

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

        if !stars.is_empty() {
            let mut buf: Vec<f32> = stars.iter().map(|s| s.fwhm).collect();
            diagnostics.median_fwhm = Some(median_mut(&mut buf));
            buf.clear();
            buf.extend(stars.iter().map(|s| s.snr));
            diagnostics.median_snr = Some(median_mut(&mut buf));
        }

        DetectionResult { stars, diagnostics }
    }
}

#[cfg(test)]
pub(super) mod internals {
    use crate::io::image::linear::LinearImage;
    use crate::stacking::star_detection::detector::{StarDetector, saturation_level};
    use crate::stacking::star_detection::resources::internals::BufferCounts;
    use crate::stacking::star_detection::resources::internals::buffer_counts;

    pub(crate) fn buffer_counts_for(detector: &StarDetector) -> Option<BufferCounts> {
        detector.resources.as_ref().map(buffer_counts)
    }

    /// The level the detector marks `image`'s pixels saturated at.
    pub(crate) fn saturation_level_of(image: &LinearImage) -> f32 {
        saturation_level(image)
    }
}

#[cfg(all(test, feature = "bench"))]
mod bench;

#[cfg(test)]
mod tests {
    use crate::stacking::star_detection::config::detection_config::DetectionConfig;
    use crate::stacking::star_detection::config::fwhm_config::FwhmMode;
    use crate::stacking::star_detection::detector::*;
    use crate::stacking::star_detection::tests::{Placement, Scenario};

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

    /// Stars up to 2.5× the 4 px seed come back at their own width: the estimate measures again
    /// at the stamp the first pass implies. Measured once at the seed's stamp they read narrow —
    /// 6.78 for a 10 px star.
    ///
    /// Flux scales with FWHM² to hold each star's peak; the brightest have SNR in the hundreds,
    /// so a star's width carries about 1% noise and the median of 13 or more well under that.
    /// 1% bounds it (measured: within 0.02%).
    #[test]
    fn auto_fwhm_recovers_wide_stars() {
        for fwhm in [5.0f32, 6.0, 8.0, 10.0] {
            let scale = fwhm * fwhm / 16.0;
            let frame = Scenario {
                size: Size2us::new(384, 384),
                num_stars: 20,
                flux: (5.0 * scale, 14.0 * scale),
                fwhm,
                placement: Placement::Uniform { margin: 40.0 },
                ..Default::default()
            }
            .frame();
            let mut config = Config::default();
            config.fwhm.mode = Some(FwhmMode::Auto { fallback: 4.0 });
            let result = StarDetector::from_config(config)
                .unwrap()
                .detect(&frame.image);
            assert!(
                matches!(result.diagnostics.fwhm, FwhmSource::Estimated { .. }),
                "FWHM {fwhm}"
            );
            let estimate = result.diagnostics.fwhm.value().unwrap();
            assert!(
                (estimate - fwhm).abs() < 0.01 * fwhm,
                "FWHM {fwhm} estimated as {estimate}"
            );
        }
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
            auto_config.fwhm.mode = Some(FwhmMode::Auto {
                fallback: configured_seed,
            });
            auto_config.fwhm.min_stars = 5;
            auto_config.filter.min_snr = 1.0;
            auto_config.filter.max_eccentricity = 1.0;
            auto_config.filter.max_sharpness = 1.0;
            auto_config.filter.max_roundness = 1.0;
            auto_config.filter.max_fwhm_deviation = None;
            auto_config.filter.duplicate_min_separation = 0.0;

            let auto_result = StarDetector::from_config(auto_config.clone())
                .unwrap()
                .detect(&frame.image);
            assert!(
                matches!(auto_result.diagnostics.fwhm, FwhmSource::Estimated { .. }),
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
            manual_config.fwhm.mode = Some(FwhmMode::Fixed(effective_fwhm));
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
