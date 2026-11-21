//! Image registration module for astronomical image alignment.
//!
//! This module provides star-based image registration using triangle matching
//! and RANSAC for robust transformation estimation.
//!
//! # Quick Start
//!
//! ```no_run
//! use lumos::{LinearImage, Registrar, RegistrationConfig, Star, WarpConfig};
//!
//! # fn example(ref_stars: &[Star], target_stars: &[Star], target_image: &LinearImage)
//! # -> Result<(), Box<dyn std::error::Error>> {
//! let registrar = Registrar::checked(RegistrationConfig::default())?;
//!
//! // Register stars from two images
//! let result = registrar.register(ref_stars, target_stars)?;
//! println!(
//!     "Matched {} stars, RMS = {:.2}px",
//!     result.matches().len(),
//!     result.rms_error_px()
//! );
//!
//! // Reproject the target onto the reference's grid, into a new image
//! let aligned = WarpConfig::default().warp(target_image, result.warp());
//! # Ok(())
//! # }
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
//! | Auto | - | Every model from Euclidean to Homography fitted, the lowest GRIC (Torr 1998) wins |
//!
//! # Configuration Presets
//!
//! - [`RegistrationConfig::default()`][default] — Balanced settings for most astrophotography
//! - [`RegistrationConfig::fast()`][fast] — Fewer iterations and stars, no local optimization
//! - [`RegistrationConfig::precise()`][precise] — More iterations, SIP distortion correction
//! - [`RegistrationConfig::wide_field()`][wide_field] — A SIP correction under the model GRIC
//!   chooses up to an affine map, for wide-field lenses
//! - [`RegistrationConfig::precise_wide_field()`][precise_wide_field] — `wide_field` with
//!   `precise`'s iteration budget and RMS gate, more stars and tighter matching
//! - [`RegistrationConfig::wide_scale()`][wide_scale] — Allows any rotation and scales from half to twice
//!
//! [default]: registration_config::RegistrationConfig::default
//! [fast]: registration_config::RegistrationConfig::fast
//! [precise]: registration_config::RegistrationConfig::precise
//! [wide_field]: registration_config::RegistrationConfig::wide_field
//! [precise_wide_field]: registration_config::RegistrationConfig::precise_wide_field
//! [wide_scale]: registration_config::RegistrationConfig::wide_scale

pub(crate) mod distortion;
pub(crate) mod error;
mod estimate;
mod final_fit;
mod kd_tree;
pub(crate) mod match_idxs;
mod model_search;
mod point_normalization;
mod point_pairs;
pub(crate) mod ransac;
mod register;
pub(crate) mod registrar;
pub(crate) mod registration_config;
pub(crate) mod registration_result;
pub(crate) mod resample;
pub(crate) mod transform;
pub(crate) mod transform_model;
pub(crate) mod triangle;
mod tuning;
pub(crate) mod warp_transform;

#[cfg(feature = "internals")]
pub(crate) mod bench;
#[cfg(test)]
mod tests;
