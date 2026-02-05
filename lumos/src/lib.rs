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
pub(crate) mod math;
pub(crate) mod registration;
pub(crate) mod stacking;
pub(crate) mod star_detection;

#[cfg(test)]
pub mod testing;

pub mod prelude;

// ============================================================================
// Core image types
// ============================================================================

pub use astro_image::{AstroImage, AstroImageMetadata, BitPix, HotPixelMap, ImageDimensions};

// ============================================================================
// Calibration
// ============================================================================

pub use calibration_masters::CalibrationMasters;

// ============================================================================
// Star detection
// ============================================================================

pub use star_detection::{
    // Configuration
    AdaptiveSigmaConfig,
    // Pipeline data structures
    BackgroundEstimate,
    BackgroundRefinement,
    // Centroid methods
    CentroidMethod,
    Config as StarDetectionConfig,
    Connectivity,
    // Sensor defects
    DefectMap,
    // Main API
    DetectionResult as StarDetectionResult,
    Diagnostics as StarDetectionDiagnostics,
    // Advanced: profile fitting
    GaussianFitConfig,
    GaussianFitResult,
    LocalBackgroundMethod,
    MoffatFitConfig,
    MoffatFitResult,
    NoiseModel,
    Star,
    StarDetector,
    alpha_beta_to_fwhm,
    fit_gaussian_2d,
    fit_moffat_2d,
    fwhm_beta_to_alpha,
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
    // Top-level functions
    register,
    register_positions,
    warp,
};

// ============================================================================
// Stacking
// ============================================================================

pub use stacking::{
    // Configuration
    CacheConfig,
    // Comet/asteroid stacking
    CometStackConfig,
    CometStackResult,
    CompositeMethod,
    // Correction method for gradient removal
    CorrectionMethod,
    FrameType,
    // Gradient removal
    GradientModel,
    GradientRemovalConfig,
    GradientRemovalError,
    GradientRemovalResult,
    // Main API
    ImageStack,
    MedianConfig,
    // Progress reporting
    ProgressCallback,
    SigmaClipConfig,
    SigmaClippedConfig,
    StackingMethod,
    StackingProgress,
    StackingStage,
    apply_comet_offset_to_transform,
    composite_stacks,
    compute_comet_offset,
    create_comet_stack_result,
    interpolate_position,
    remove_gradient,
    remove_gradient_image,
    remove_gradient_simple,
};
