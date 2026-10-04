//! Star detector implementation and related types.
//!
//! This module contains the main [`StarDetector`] struct and its associated
//! types for detecting stars in astronomical images.

pub(super) mod stages;

use serde::{Deserialize, Serialize};

use crate::io::image::linear::LinearImage;
use crate::math::size2us::Size2us;

use crate::error::InvalidConfigField;
use crate::math::statistics::median_mut;
use crate::star_detection::config::Config;
use crate::star_detection::detector::stages::detect::DetectResult;
use crate::star_detection::detector::stages::filter::FilterOutcome;
use crate::star_detection::detector::stages::fwhm;
use crate::star_detection::detector::stages::prepared_frame::PreparedFrame;
use crate::star_detection::resources::DetectionResources;
use crate::star_detection::star::Star;

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
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
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
            FwhmSource::Configured(fwhm) | FwhmSource::Estimated { fwhm, .. } => Some(*fwhm),
        }
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
            &self.config.detection,
            resources,
        );
        plane.release_to_pool(resources);

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
    use crate::star_detection::detector::StarDetector;
    use crate::star_detection::resources::internals::BufferCounts;
    use crate::star_detection::resources::internals::buffer_counts;

    pub(crate) fn buffer_counts_for(detector: &StarDetector) -> Option<BufferCounts> {
        detector.resources.as_ref().map(buffer_counts)
    }
}

#[cfg(all(test, feature = "bench"))]
mod bench;

#[cfg(test)]
mod tests {
    use crate::star_detection::config::detection_config::DetectionConfig;
    use crate::star_detection::config::fwhm_config::FwhmMode;
    use crate::star_detection::detector::*;
    use crate::star_detection::tests::{Placement, Scenario};

    /// Review item 9.1. A Gaussian star of FWHM 2 keeps all its flux on a demosaiced frame: the
    /// detection plane takes the 3×3 median there, and measurement reads the plane no filter
    /// touched, so the frame read as demosaiced and as measured gives the same star, bit for bit.
    /// The median used to keep 56% of it.
    ///
    /// Amplitude 1 on a sky of 0.1, the same in all three channels with a noise of 0.001. The flux
    /// is the rendered star's sum, 4.53, plus the noise over the 15 × 15 stamp of FWHM 4,
    /// 0.015 of standard deviation: within 5 of them.
    #[test]
    fn a_demosaiced_frame_measures_its_stars_unfiltered() {
        use crate::internals::prelude::*;
        use crate::internals::synthetic::star_profiles::{StarProfile, SyntheticStar};
        use crate::io::image::image_provenance::{
            ColorProvenance, DecoderProvenance, DemosaicProvenance, ImageProvenance, RowOrder,
            SourceContainer, TransferProvenance,
        };

        let size = Size2us::new(64, 64);
        let mut star = Buffer2::new_filled(size.width, size.height, 0.0f32);
        SyntheticStar::new(
            Vec2::new(32.3, 31.6),
            1.0,
            StarProfile::Gaussian {
                sigma: 2.0 / 2.354_82,
            },
        )
        .add_to(&mut star);
        let truth: f64 = star.pixels().iter().map(|&value| f64::from(value)).sum();
        let mut rng = TestRng::new(3);
        let channel: Vec<f32> = star
            .pixels()
            .iter()
            .map(|&value| 0.1 + value + 0.001 * rng.next_gaussian_f32())
            .collect();
        let frame = |demosaic| {
            let mut image = rgb_image(size, channel.clone(), channel.clone(), channel.clone());
            image.metadata.provenance = Some(ImageProvenance {
                container: SourceContainer::CameraRaw,
                decoder: DecoderProvenance::LibRaw,
                transfer: TransferProvenance::RawNormalized,
                color: ColorProvenance::SensorRgb,
                clipped: true,
                demosaic,
                row_order: RowOrder::TopDown,
            });
            image
        };
        // A star this narrow, off the pixel grid, reads lopsided marginals; its shape is not what
        // this measures.
        let mut config = Config::default();
        config.filter.max_roundness = 2.0;
        let brightest = |image: &LinearImage| {
            StarDetector::from_config(config.clone())
                .unwrap()
                .detect(image)
                .stars
                .into_iter()
                .next()
                .expect("the star is detected")
        };
        let demosaiced = brightest(&frame(DemosaicProvenance::LumosRcd));
        let measured = brightest(&frame(DemosaicProvenance::None));
        assert_eq!(demosaiced.pos, measured.pos);
        assert_eq!(demosaiced.flux.to_bits(), measured.flux.to_bits());
        assert_eq!(demosaiced.peak.to_bits(), measured.peak.to_bits());
        assert_eq!(demosaiced.fwhm.to_bits(), measured.fwhm.to_bits());
        assert!(
            (f64::from(demosaiced.flux) - truth).abs() <= 5.0 * 0.015,
            "flux {} of {truth}",
            demosaiced.flux
        );
    }

    /// Review phase 7 and S9: detection does not depend on the frame's scale, its zero point or
    /// its orientation. The star field is put on the exact grid of `Affine::CASES`; its saturation
    /// is flagged by the decoder, with no flags, so no level stated in the frame's units enters.
    /// - Under a power-of-two scale every sum, median and product scales exactly, so the stars
    ///   are the same, bit for bit, their flux scaled.
    /// - Under a pedestal each sample rounds by up to half an ulp of the largest sample, `u`,
    ///   before the sky takes the pedestal out. Against the faintest star's peak `A`, scaled, that
    ///   moves a centroid by at most the stamp radius, 7 px at FWHM 4, times `u / (s·A)`. The sky
    ///   itself is computed at the pedestal's magnitude in f32 — the Pearson mode `2.5·median −
    ///   1.5·mean`, the plane fit, the bicubic spline — and rounds by a few ulps of it, which every
    ///   pixel of a flux's 15 × 15 stamp carries alike: 32 `u` per pixel bound a flux's change by
    ///   `225·32·u / (s·F)` of the faintest flux `F`.
    /// - Rotated by 180°, on a frame of whole tiles, the mesh's tiles and samples map onto
    ///   themselves, so every star maps onto its own; only the order of a few sums changes, which
    ///   moves a centroid by under 1e-9 px.
    #[test]
    fn detection_is_invariant_to_scale_zero_point_and_rotation() {
        use crate::internals::invariance::Affine;
        use crate::internals::prelude::*;

        let frame = Scenario::default().frame();
        let size = Size2us::new(frame.image.width(), frame.image.height());
        let base: Vec<f32> = frame
            .image
            .channel(0)
            .pixels()
            .iter()
            .map(|&value| Affine::quantize(value))
            .collect();
        let detect = |pixels: Vec<f32>| {
            let mut image = gray_image(size, pixels);
            image.metadata.saturation_flagged = true;
            StarDetector::default().detect(&image).stars
        };
        let reference = detect(base.clone());
        assert!(reference.len() > 10, "{}", reference.len());
        let faintest =
            |of: fn(&Star) -> f32| reference.iter().map(of).fold(f32::INFINITY, f32::min);
        let (peak, flux) = (faintest(|star| star.peak), faintest(|star| star.flux));

        for case in Affine::CASES {
            let stars = detect(case.apply_all(&base));
            assert_eq!(stars.len(), reference.len(), "{case:?}");
            for (star, expected) in stars.iter().zip(&reference) {
                let ratio = star.flux / case.scale / expected.flux - 1.0;
                if case.offset == 0.0 {
                    assert_eq!(star.pos, expected.pos, "{case:?}");
                    assert_eq!(ratio, 0.0, "{case:?}");
                } else {
                    let rounding = f64::from((case.offset + case.scale) * f32::EPSILON / 2.0);
                    let moved = (star.pos - expected.pos).length();
                    assert!(
                        moved <= 7.0 * rounding / f64::from(case.scale * peak),
                        "{case:?}: {moved}"
                    );
                    assert!(
                        f64::from(ratio.abs())
                            <= 225.0 * 32.0 * rounding / f64::from(case.scale * flux),
                        "{case:?}: {ratio}"
                    );
                }
            }
        }

        let flip = |pos: DVec2| {
            DVec2::new(
                size.width as f64 - 1.0 - pos.x,
                size.height as f64 - 1.0 - pos.y,
            )
        };
        let order = |a: &DVec2, b: &DVec2| a.y.total_cmp(&b.y).then(a.x.total_cmp(&b.x));
        let mut rotated: Vec<DVec2> = detect(base.iter().rev().copied().collect())
            .iter()
            .map(|star| flip(star.pos))
            .collect();
        let mut expected: Vec<DVec2> = reference.iter().map(|star| star.pos).collect();
        rotated.sort_by(order);
        expected.sort_by(order);
        assert_eq!(rotated.len(), expected.len());
        for (rotated, expected) in rotated.iter().zip(&expected) {
            assert!(
                (*rotated - *expected).length() < 1e-9,
                "{rotated} vs {expected}"
            );
        }
    }

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
            auto_config.filter.max_roundness = 2.0;
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
