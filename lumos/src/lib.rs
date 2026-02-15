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
pub use astro_image::error::ImageLoadError;
pub use astro_image::{AstroImage, AstroImageMetadata, BitPix, ImageDimensions};
pub use calibration_masters::DefectMap;
pub use raw::demosaic::CfaPattern;

// ============================================================================
// Calibration
// ============================================================================

pub use calibration_masters::{CalibrationMasters, DEFAULT_HOT_PIXEL_SIGMA};

// ============================================================================
// Star detection
// ============================================================================

pub use star_detection::{
    // Pipeline data structures
    BackgroundEstimate,
    BackgroundRefinement,
    // Configuration
    CentroidMethod,
    ChannelStats,
    Config as StarDetectionConfig,
    Connectivity,
    // Main API
    DetectionResult as StarDetectionResult,
    Diagnostics as StarDetectionDiagnostics,
    LocalBackgroundMethod,
    NoiseModel,
    Star,
    StarDetector,
};

// ============================================================================
// Registration
// ============================================================================

pub use registration::{
    // Configuration
    Config as RegistrationConfig,
    InterpolationMethod,
    // Results and errors
    RansacFailureReason,
    RegistrationError,
    RegistrationResult,
    // Core types
    SipPolynomial,
    Transform,
    TransformType,
    WarpTransform,
    // Top-level functions
    register,
    warp,
};

// ============================================================================
// Stacking
// ============================================================================

pub use stacking::{
    // Configuration
    CacheConfig,
    CombineMethod,
    FrameType,
    Normalization,
    // Progress reporting
    ProgressCallback,
    Rejection,
    StackConfig,
    StackingProgress,
    StackingStage,
    // Main API
    stack,
    stack_with_progress,
};

// ============================================================================
// Drizzle
// ============================================================================

pub use drizzle::{DrizzleAccumulator, DrizzleConfig, DrizzleKernel, DrizzleResult, drizzle_stack};
