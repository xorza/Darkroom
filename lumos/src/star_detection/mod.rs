//! Star detection and centroid computation for image registration.
//!
//! This module detects stars in astronomical images and computes sub-pixel
//! accurate centroids for use in image alignment and stacking.
//!
//! # Algorithm Overview
//!
//! 1. **Background estimation**: Divide image into tiles, compute sigma-clipped
//!    median per tile, then bilinearly interpolate to create a smooth background map.
//!
//! 2. **Star detection**: Threshold pixels above background + k×σ, then use
//!    connected component labeling to group pixels into candidate stars.
//!
//! 3. **Filtering**: Reject candidates that are too small, too large, elongated,
//!    near edges, or saturated.
//!
//! 4. **Sub-pixel centroid**: Compute precise centroid using iterative weighted
//!    centroid algorithm (achieves ~0.05 pixel accuracy).
//!
//! 5. **Quality metrics**: Compute FWHM, SNR, and eccentricity for each star.

// =============================================================================
// Submodules
// =============================================================================

pub(crate) mod background;
mod buffer_pool;
pub(crate) mod candidate_detection;
mod centroid;
pub mod config;
mod convolution;
mod cosmic_ray;
mod deblend;
mod defect_map;
mod detector;
mod fwhm_estimation;
pub mod image_stats;
mod mask_dilation;
mod median_filter;
mod region;
pub(crate) mod stages;
mod star;
pub(crate) mod threshold_mask;

#[cfg(test)]
pub mod tests;

// =============================================================================
// Public API Exports
// =============================================================================

// Main detector types
pub use buffer_pool::BufferPool;
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

// Background estimation (kept during migration, will become ImageStats)
pub use background::BackgroundMap;

// Pipeline data structures
pub use image_stats::ImageStats;
pub use region::Region;

// Centroid methods
pub use centroid::{GaussianFitConfig, GaussianFitResult, fit_gaussian_2d};
pub use centroid::{
    MoffatFitConfig, MoffatFitResult, alpha_beta_to_fwhm, fit_moffat_2d, fwhm_beta_to_alpha,
};
