//! Star detection and centroid computation for image registration.
//!
//! This module detects stars in astronomical images and computes sub-pixel
//! accurate centroids for use in image alignment and stacking.
//!
//! # Pipeline Overview
//!
//! The detection pipeline consists of 6 stages:
//!
//! 1. **Prepare**: Combine the channels, each weighted by its noise, into one plane; mark the
//!    saturated pixels and those with no data. A demosaiced frame's detection plane takes a 3×3
//!    median first; the plane measurement reads is never filtered.
//!
//! 2. **Background**: Estimate per-pixel background and noise using tiled
//!    sigma-clipped statistics with natural-cubic-spline interpolation. Optional iterative
//!    refinement masks the detected sources and estimates again, for
//!    nebulous fields.
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
//! ```no_run
//! use lumos::detection::{self, StarDetector};
//! use lumos::{InvalidConfigField, LinearImage};
//!
//! # fn example(image: &LinearImage) -> Result<(), InvalidConfigField> {
//! // Use a preset configuration
//! let config = detection::Config::wide_field();
//!
//! // Or customize from defaults
//! let mut config = detection::Config::default();
//! config.filter.min_snr = 15.0;
//! config.detection.sigma_threshold = 3.0;
//!
//! // Detect stars
//! let mut detector = StarDetector::from_config(config)?;
//! let result = detector.detect(image);
//!
//! println!("Found {} stars", result.stars.len());
//! # Ok(())
//! # }
//! ```

pub(crate) mod background;
mod centroid;
pub(crate) mod config;
mod convolution;
mod deblend;
pub(crate) mod detection_plane;
pub(crate) mod detector;
mod labeling;
mod median_filter;
pub(crate) mod resources;
pub(crate) mod roundness;
pub(crate) mod star;
mod threshold_mask;

#[cfg(test)]
mod tests;
