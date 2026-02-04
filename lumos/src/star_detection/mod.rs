//! Star detection and centroid computation for image registration.
//!
//! This module detects stars in astronomical images and computes sub-pixel
//! accurate centroids for use in image alignment and stacking.
//!
//! # Pipeline Overview
//!
//! The detection pipeline consists of 6 stages:
//!
//! 1. **Prepare**: Convert to grayscale, apply defect map correction, median
//!    filter for CFA images.
//!
//! 2. **Background**: Estimate per-pixel background and noise using tiled
//!    sigma-clipped statistics with bilinear interpolation. Optional iterative
//!    refinement or adaptive thresholding for nebulous fields.
//!
//! 3. **FWHM Estimation**: Optionally auto-estimate PSF FWHM from bright stars
//!    for matched filtering.
//!
//! 4. **Detect**: Threshold pixels above background + k×σ, connected component
//!    labeling, deblending (local maxima or multi-threshold), and region
//!    filtering (size, edge margin).
//!
//! 5. **Measure**: Compute sub-pixel centroids using weighted moments or
//!    Gaussian/Moffat profile fitting, plus quality metrics (flux, FWHM,
//!    eccentricity, SNR, sharpness, roundness).
//!
//! 6. **Filter**: Apply quality thresholds (SNR, eccentricity, sharpness,
//!    roundness), remove FWHM outliers and duplicates, sort by flux.
//!
//! # Example
//!
//! ```rust,ignore
//! use lumos::star_detection::{Config, StarDetector};
//!
//! // Use a preset configuration
//! let config = Config::wide_field();
//!
//! // Or customize from defaults
//! let mut config = Config::default();
//! config.min_snr = 15.0;
//! config.sigma_threshold = 3.0;
//!
//! // Detect stars
//! let mut detector = StarDetector::from_config(config);
//! let result = detector.detect(&image);
//!
//! println!("Found {} stars", result.stars.len());
//! ```

// =============================================================================
// Submodules
// =============================================================================

pub(crate) mod background;
mod buffer_pool;
mod centroid;
pub mod config;
mod convolution;
mod cosmic_ray;
mod deblend;
mod defect_map;
pub(crate) mod detector;
pub(crate) mod labeling;
mod mask_dilation;
mod median_filter;
mod star;
pub(crate) mod threshold_mask;

#[cfg(test)]
pub mod tests;

// =============================================================================
// Public API Exports
// =============================================================================

// Main detector types
pub(crate) use buffer_pool::BufferPool;
pub use detector::{DetectionResult, Diagnostics, StarDetector};

// Configuration
pub use config::CentroidMethod;
pub use config::Config;
pub use config::Connectivity;
pub use config::LocalBackgroundMethod;
pub use config::NoiseModel;
pub use config::{AdaptiveSigmaConfig, BackgroundRefinement};
pub use defect_map::DefectMap;
pub use star::Star;

// Pipeline data structures
pub use background::BackgroundEstimate;
pub use deblend::Region;

// Centroid methods
pub use centroid::{GaussianFitConfig, GaussianFitResult, fit_gaussian_2d};
pub use centroid::{
    MoffatFitConfig, MoffatFitResult, alpha_beta_to_fwhm, fit_moffat_2d, fwhm_beta_to_alpha,
};
