//! Configuration types for star detection.
//!
//! This module defines the composed [`StarDetectionConfig`] and stage-specific configuration types
//! used by the star detection pipeline.

use std::num::NonZeroUsize;

use crate::error::InvalidConfigField;

pub(crate) mod bg_config;
pub(crate) mod extraction_config;
pub(crate) mod filter_config;
pub(crate) mod fwhm_config;
pub(crate) mod measurement_config;

use crate::star_detection::star_detection_config::bg_config::{BgConfig, IterativeRefinement};
use crate::star_detection::star_detection_config::extraction_config::{Deblend, ExtractionConfig};
use crate::star_detection::star_detection_config::filter_config::FilterConfig;
use crate::star_detection::star_detection_config::fwhm_config::{FwhmConfig, FwhmMode};
use crate::star_detection::star_detection_config::measurement_config::{
    CentroidMethod, LocalBgMethod, MeasurementConfig,
};

/// Configuration for the star detection pipeline, composed by processing stage.
///
/// # Example
///
/// ```
/// use lumos::StarDetectionConfig;
///
/// // Use a preset
/// let config = StarDetectionConfig::wide_field();
///
/// // Customize from a preset
/// let mut config = StarDetectionConfig::crowded_field();
/// config.filter.min_snr = 20.0;
/// ```
#[derive(Debug, Clone, Default)]
pub struct StarDetectionConfig {
    /// Background estimation and refinement settings.
    pub bg: BgConfig,
    /// Candidate detection, deblending, and region-filtering settings.
    pub extraction: ExtractionConfig,
    /// Matched-filter FWHM selection and estimation settings.
    pub fwhm: FwhmConfig,
    /// Centroid and metric measurement settings.
    pub measurement: MeasurementConfig,
    /// Final quality and duplicate-filtering settings.
    pub filter: FilterConfig,
}

impl StarDetectionConfig {
    /// Validate every parameter before constructing a detector.
    pub(crate) fn validate(&self) -> Result<(), InvalidConfigField> {
        self.bg.validate()?;
        self.extraction.validate()?;
        self.fwhm.validate()?;
        self.measurement.validate()?;
        self.filter.validate()?;
        Ok(())
    }

    /// Settings for wide stars, FWHM 5–8 px: oversampled seeing — a long focal length over small
    /// pixels spreads a star over more of them — or a fast optic whose coma and field curvature
    /// widen and elongate the stars toward the field's edge. Uses relaxed eccentricity filtering,
    /// auto FWHM estimation from a 6 px start, and larger area and edge limits.
    pub fn wide_field() -> Self {
        Self {
            fwhm: FwhmConfig {
                mode: Some(FwhmMode::Auto { fallback: 6.0 }),
                min_stars: 15,
                ..Default::default()
            },
            extraction: ExtractionConfig {
                min_area: 7,
                edge_margin: 20,
                ..Default::default()
            },
            filter: FilterConfig {
                max_eccentricity: 0.7,
                ..Default::default()
            },
            ..Self::default()
        }
    }

    /// Settings for compact stars, FWHM 2–4 px: critically sampled, Nyquist PSFs of symmetric
    /// profile, as a focal length matched to the seeing gives — a shorter one over the same pixels
    /// spreads a star over fewer. Uses Gaussian centroid fitting for maximum precision, stricter
    /// eccentricity and roundness filtering, and a higher SNR threshold to build a clean catalog.
    pub fn high_resolution() -> Self {
        Self {
            fwhm: FwhmConfig {
                mode: Some(FwhmMode::Auto { fallback: 2.5 }),
                min_stars: 15,
                ..Default::default()
            },
            extraction: ExtractionConfig {
                min_area: 3,
                ..Default::default()
            },
            measurement: MeasurementConfig {
                centroid_method: CentroidMethod::GaussianFit,
                ..Default::default()
            },
            filter: FilterConfig {
                min_snr: 15.0,
                max_eccentricity: 0.5,
                max_roundness: 0.3,
                ..Default::default()
            },
            ..Self::default()
        }
    }

    /// Crowded field settings (globular clusters, dense star fields).
    ///
    /// Enables SExtractor-style multi-threshold deblending (32 sub-thresholds)
    /// with low contrast threshold to separate close blends. Uses iterative
    /// background refinement to re-estimate background after masking sources.
    pub fn crowded_field() -> Self {
        Self {
            bg: BgConfig {
                refinement: Some(IterativeRefinement {
                    iterations: NonZeroUsize::new(2).unwrap(),
                    mask_dilation: 3,
                    mask_sigma: 2.0,
                }),
                ..Default::default()
            },
            extraction: ExtractionConfig {
                deblend: Deblend::MultiThreshold {
                    threshold_count: 32,
                    min_contrast: 0.005,
                },
                deblend_min_separation: 2,
                ..Default::default()
            },
            fwhm: FwhmConfig {
                mode: Some(FwhmMode::Auto { fallback: 4.0 }),
                ..Default::default()
            },
            filter: FilterConfig {
                duplicate_min_separation: 3.0,
                ..Default::default()
            },
            ..Self::default()
        }
    }

    /// Maximum centroid precision settings for ground-based astrophotography.
    ///
    /// Optimized for sub-pixel astrometric accuracy. Uses Moffat PSF fitting
    /// (beta=2.5) which models atmospheric seeing wings better than Gaussian.
    /// Local annulus background subtraction handles nebulosity near stars.
    pub fn precise_ground() -> Self {
        Self {
            bg: BgConfig {
                tile_size: 128,
                sigma_clip_max_iterations: 3,
                refinement: Some(IterativeRefinement {
                    iterations: NonZeroUsize::new(3).unwrap(),
                    mask_dilation: 5,
                    mask_sigma: 2.0,
                }),
            },
            extraction: ExtractionConfig {
                sigma_threshold: 3.0,
                min_area: 7,
                edge_margin: 15,
                deblend: Deblend::MultiThreshold {
                    threshold_count: 32,
                    min_contrast: 0.003,
                },
                deblend_min_separation: 2,
                ..Default::default()
            },
            fwhm: FwhmConfig {
                mode: Some(FwhmMode::Auto { fallback: 3.0 }),
                min_stars: 30,
                estimation_sigma_factor: 2.5,
                ..Default::default()
            },
            measurement: MeasurementConfig {
                centroid_method: CentroidMethod::MoffatFit { beta: 2.5 },
                local_bg: LocalBgMethod::LocalAnnulus,
                ..Default::default()
            },
            filter: FilterConfig {
                min_snr: 15.0,
                max_fwhm_deviation: Some(4.0),
                duplicate_min_separation: 5.0,
                ..Default::default()
            },
        }
    }
}

#[cfg(test)]
mod tests;
