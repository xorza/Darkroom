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
//! config.filter.min_snr = 15.0;
//! config.detection.sigma_threshold = 3.0;
//!
//! // Detect stars
//! let mut detector = StarDetector::from_config(config)?;
//! let result = detector.detect(&image);
//!
//! println!("Found {} stars", result.stars.len());
//! ```

pub(crate) mod background;
pub(crate) mod centroid;
pub(crate) mod config;
mod convolution;
pub(crate) mod deblend;
pub(crate) mod detector;
pub(crate) mod error;
pub(crate) mod labeling;
mod mask_dilation;
mod median_filter;
pub(crate) mod resources;
pub(crate) mod star;
pub(crate) mod threshold_mask;

#[cfg(test)]
mod mem_budget_probe;
#[cfg(test)]
mod mem_budget_tests;
#[cfg(all(test, feature = "real-data"))]
mod real_data_tests;
#[cfg(test)]
mod synthetic_tests;
#[cfg(test)]
pub(crate) mod test_common;
