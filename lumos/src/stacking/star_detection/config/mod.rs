//! Configuration types for star detection.
//!
//! This module defines the composed [`Config`] and stage-specific configuration types used by
//! the star detection pipeline.

use crate::error::InvalidConfigField;

pub(crate) mod background_config;
pub(crate) mod detection_config;
pub(crate) mod filter_config;
pub(crate) mod fwhm_config;
pub(crate) mod measurement_config;

use crate::stacking::star_detection::config::background_config::{
    BackgroundConfig, BackgroundRefinement,
};
use crate::stacking::star_detection::config::detection_config::{Connectivity, DetectionConfig};
use crate::stacking::star_detection::config::filter_config::FilterConfig;
use crate::stacking::star_detection::config::fwhm_config::FwhmConfig;
use crate::stacking::star_detection::config::measurement_config::{
    CentroidMethod, LocalBackgroundMethod, MeasurementConfig,
};

/// Configuration for the star detection pipeline, composed by processing stage.
///
/// # Example
///
/// ```rust,ignore
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
pub struct Config {
    /// Background estimation and refinement settings.
    pub background: BackgroundConfig,
    /// Candidate detection, deblending, and region-filtering settings.
    pub detection: DetectionConfig,
    /// Matched-filter FWHM selection and estimation settings.
    pub fwhm: FwhmConfig,
    /// Centroid and metric measurement settings.
    pub measurement: MeasurementConfig,
    /// Final quality and duplicate-filtering settings.
    pub filter: FilterConfig,
}

impl Config {
    /// Validate every parameter before constructing a detector.
    pub fn validate(&self) -> Result<(), InvalidConfigField> {
        self.background.validate()?;
        self.detection.validate()?;
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
                expected: 6.0,
                auto_estimate: true,
                min_stars: 15,
                ..Default::default()
            },
            detection: DetectionConfig {
                min_area: 7,
                max_area: 1500,
                edge_margin: 20,
                connectivity: Connectivity::Eight,
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
                expected: 2.5,
                auto_estimate: true,
                min_stars: 15,
                ..Default::default()
            },
            detection: DetectionConfig {
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
            background: BackgroundConfig {
                refinement: BackgroundRefinement::Iterative { iterations: 2 },
                ..Default::default()
            },
            detection: DetectionConfig {
                deblend_n_thresholds: 32,
                deblend_min_separation: 2,
                deblend_min_prominence: 0.15,
                deblend_min_contrast: 0.005,
                connectivity: Connectivity::Eight,
                ..Default::default()
            },
            fwhm: FwhmConfig {
                auto_estimate: true,
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
            background: BackgroundConfig {
                mask_dilation: 5,
                tile_size: 128,
                sigma_clip_iterations: 3,
                refinement: BackgroundRefinement::Iterative { iterations: 3 },
            },
            detection: DetectionConfig {
                sigma_threshold: 3.0,
                min_area: 7,
                max_area: 2000,
                edge_margin: 15,
                connectivity: Connectivity::Eight,
                deblend_min_separation: 2,
                deblend_min_prominence: 0.15,
                deblend_n_thresholds: 32,
                deblend_min_contrast: 0.003,
                ..Default::default()
            },
            fwhm: FwhmConfig {
                expected: 3.0,
                auto_estimate: true,
                min_stars: 30,
                estimation_sigma_factor: 2.5,
            },
            measurement: MeasurementConfig {
                centroid_method: CentroidMethod::MoffatFit { beta: 2.5 },
                local_background: LocalBackgroundMethod::LocalAnnulus,
                ..Default::default()
            },
            filter: FilterConfig {
                min_snr: 15.0,
                max_fwhm_deviation: 4.0,
                duplicate_min_separation: 5.0,
                ..Default::default()
            },
        }
    }
}

#[cfg(test)]
mod tests;
