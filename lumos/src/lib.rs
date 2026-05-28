//! Lumos - Astronomical image processing library.
//!
//! This library provides tools for processing astronomical images, including:
//! - Star detection and centroiding
//! - Image registration and alignment
//! - Frame stacking (mean, median, sigma-clipped)
//! - Calibration frame handling (darks, flats, bias)
//!
//! # Quick Start
//!
//! ```rust,ignore
//! use lumos::{AstroImage, StarDetectionConfig, StarDetector};
//!
//! // Load an astronomical image
//! let image = AstroImage::from_file("light_001.fits")?;
//!
//! // Detect stars
//! let config = StarDetectionConfig::default();
//! let mut detector = StarDetector::from_config(config);
//! let result = detector.detect(&image);
//!
//! println!("Found {} stars", result.stars.len());
//! ```

mod astro_image;
mod calibration_masters;
pub(crate) mod common;
pub mod drizzle;
pub(crate) mod math;
pub mod raw;
pub(crate) mod registration;
pub(crate) mod stacking;
pub(crate) mod star_detection;

#[cfg(test)]
pub mod testing;

pub mod prelude;

// ============================================================================
// Core image types
// ============================================================================

pub use astro_image::cfa::{CfaImage, CfaType};
pub use astro_image::error::ImageError;
pub use astro_image::{AstroImage, AstroImageMetadata, BitPix, ImageDimensions};
pub use calibration_masters::defect_map::DefectMap;
pub use raw::demosaic::bayer::CfaPattern;

// ============================================================================
// Calibration
// ============================================================================

pub use calibration_masters::{CalibrationFrames, CalibrationMasters, DEFAULT_SIGMA_THRESHOLD};

// ============================================================================
// Star detection
// ============================================================================

pub use math::statistics::ChannelStats;
pub use star_detection::background::estimate::BackgroundEstimate;
pub use star_detection::config::{
    BackgroundRefinement, CentroidMethod, Config as StarDetectionConfig, Connectivity,
    LocalBackgroundMethod, NoiseModel,
};
pub use star_detection::detector::{
    DetectionResult as StarDetectionResult, Diagnostics as StarDetectionDiagnostics, StarDetector,
};
pub use star_detection::star::Star;

// ============================================================================
// Registration
// ============================================================================

pub use registration::config::{Config as RegistrationConfig, InterpolationMethod};
pub use registration::distortion::sip::SipPolynomial;
pub use registration::result::{RansacFailureReason, RegistrationError, RegistrationResult};
pub use registration::transform::{Transform, TransformType, WarpTransform};
pub use registration::{register, warp};

// ============================================================================
// Stacking
// ============================================================================

pub use stacking::cache_config::CacheConfig;
pub use stacking::config::{CombineMethod, Normalization, StackConfig};
pub use stacking::error::Error as StackError;
pub use stacking::progress::{ProgressCallback, StackingProgress, StackingStage};
pub use stacking::rejection::Rejection;
pub use stacking::stack::{stack, stack_images, stack_images_with_progress, stack_with_progress};

// ============================================================================
// Drizzle
// ============================================================================

pub use drizzle::error::DrizzleError;
pub use drizzle::{
    DrizzleAccumulator, DrizzleConfig, DrizzleKernel, DrizzleResult, drizzle_images, drizzle_stack,
};
