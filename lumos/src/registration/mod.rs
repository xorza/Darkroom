//! Image registration module for astronomical image alignment.
//!
//! This module provides star-based image registration using triangle matching
//! and RANSAC for robust transformation estimation.
//!
//! # Quick Start
//!
//! ```ignore
//! use lumos::registration::{register, warp, Config};
//!
//! // Register stars from two images
//! let result = register(&ref_stars, &target_stars, &Config::default())?;
//! println!("Matched {} stars, RMS = {:.2}px", result.num_inliers, result.rms_error);
//!
//! // Warp target image to align with reference
//! let aligned = warp(&target_image, &result.transform, &Config::default());
//! ```
//!
//! # Transformation Models
//!
//! | Type | DOF | Description |
//! |------|-----|-------------|
//! | Translation | 2 | X/Y offset only |
//! | Euclidean | 3 | Translation + rotation |
//! | Similarity | 4 | Translation + rotation + uniform scale |
//! | Affine | 6 | Handles shear and differential scaling |
//! | Homography | 8 | Full perspective transformation |
//! | Auto | - | Starts with Similarity, upgrades to Homography if needed |
//!
//! # Configuration Presets
//!
//! - [`Config::default()`] — Balanced settings for most astrophotography
//! - [`Config::fast()`] — Fewer iterations, bilinear interpolation
//! - [`Config::precise()`] — More iterations, SIP distortion correction
//! - [`Config::wide_field()`] — Homography + SIP for wide-field lenses
//! - [`Config::mosaic()`] — Allows larger rotations and scale differences

pub(crate) mod config;
pub(crate) mod distortion;
pub(crate) mod interpolation;
pub(crate) mod pipeline;
pub(crate) mod quality;
pub(crate) mod ransac;
pub(crate) mod spatial;
pub(crate) mod transform;
pub(crate) mod triangle;

#[cfg(test)]
mod tests;

// === Primary Public API ===

// Configuration
pub use config::{Config, InterpolationMethod};

// Core types
pub use transform::{Transform, TransformType};

// Results and errors
pub use pipeline::{RansacFailureReason, RegistrationError, RegistrationResult, Registrator};

// Distortion (for users who need manual SIP access)
pub use distortion::SipPolynomial;

// === Top-Level Functions ===

use crate::AstroImage;
use crate::star_detection::Star;
use glam::DVec2;

/// Register two sets of star positions.
///
/// This is the main entry point for image registration. It finds the geometric
/// transformation that maps reference star positions to target star positions.
///
/// Stars should be sorted by brightness (flux) in descending order for best results.
///
/// # Example
///
/// ```ignore
/// use lumos::registration::{register, Config, TransformType};
///
/// // With defaults
/// let result = register(&ref_stars, &target_stars, &Config::default())?;
///
/// // With custom config
/// let config = Config {
///     transform_type: TransformType::Similarity,
///     inlier_threshold: 3.0,
///     ..Config::default()
/// };
/// let result = register(&ref_stars, &target_stars, &config)?;
///
/// println!("Matched {} stars", result.num_inliers);
/// println!("RMS error: {:.2} pixels", result.rms_error);
/// ```
pub fn register(
    ref_stars: &[Star],
    target_stars: &[Star],
    config: &Config,
) -> Result<RegistrationResult, RegistrationError> {
    let registrator = Registrator::new(config.clone());
    registrator.register_stars(ref_stars, target_stars)
}

/// Register using raw position vectors instead of Star structs.
///
/// Useful when you have pre-extracted positions or for testing.
/// Positions should be sorted by brightness (brightest first) for best results.
pub fn register_positions(
    ref_positions: &[DVec2],
    target_positions: &[DVec2],
    config: &Config,
) -> Result<RegistrationResult, RegistrationError> {
    let registrator = Registrator::new(config.clone());
    registrator.register_positions(ref_positions, target_positions)
}

/// Warp an image to align with the reference frame.
///
/// Takes a target image and applies the inverse transformation so it aligns
/// pixel-for-pixel with the reference image.
///
/// # Example
///
/// ```ignore
/// use lumos::registration::{register, warp, Config};
///
/// let result = register(&ref_stars, &target_stars, &Config::default())?;
/// let aligned = warp(&target_image, &result.transform, &Config::default());
/// ```
pub fn warp(image: &AstroImage, transform: &Transform, config: &Config) -> AstroImage {
    pipeline::warp_to_reference_image(image, transform, config.interpolation)
}

// === Internal Re-exports (for submodules) ===

// These are pub(crate) so internal code can use them without
// going through the full path, but they're not part of the public API.

pub(crate) use distortion::SipConfig;
pub(crate) use interpolation::warp_image;
pub(crate) use ransac::{RansacEstimator, RansacResult};
pub(crate) use triangle::{PointMatch, match_triangles};
