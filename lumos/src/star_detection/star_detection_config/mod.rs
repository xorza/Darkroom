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

use crate::star_detection::star_detection_config::bg_config::{
    BgConfig, BgRefinement, IterativeRefinement,
};
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

    /// Wide-field imaging settings (short focal length, large pixel scale).
    ///
    /// Wide-field setups produce larger stars (FWHM 5-8px) that may be slightly
    /// elongated at field edges due to coma and field curvature. Uses relaxed
    /// eccentricity filtering, auto FWHM estimation, and 8-connectivity for
    /// undersampled PSFs that may not connect well with 4-connectivity.
    pub fn wide_field() -> Self {
        Self {
            fwhm: FwhmConfig {
                mode: Some(FwhmMode::Auto { fallback: 6.0 }),
                min_stars: 15,
                ..Default::default()
            },
            extraction: ExtractionConfig {
                min_area: 7,
                max_area: 1500,
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

    /// High-resolution imaging settings (long focal length, small pixel scale).
    ///
    /// Well-sampled Nyquist PSFs (FWHM 2-4px) with symmetric profiles. Uses
    /// Gaussian centroid fitting for maximum precision on well-sampled stars,
    /// stricter eccentricity and roundness filtering, and higher SNR threshold
    /// to build a clean, high-quality star catalog.
    pub fn high_resolution() -> Self {
        Self {
            fwhm: FwhmConfig {
                mode: Some(FwhmMode::Auto { fallback: 2.5 }),
                min_stars: 15,
                ..Default::default()
            },
            extraction: ExtractionConfig {
                min_area: 3,
                max_area: 200,
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
                refinement: BgRefinement::Iterative(IterativeRefinement {
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
                refinement: BgRefinement::Iterative(IterativeRefinement {
                    iterations: NonZeroUsize::new(3).unwrap(),
                    mask_dilation: 5,
                    mask_sigma: 2.0,
                }),
            },
            extraction: ExtractionConfig {
                sigma_threshold: 3.0,
                min_area: 7,
                max_area: 2000,
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
