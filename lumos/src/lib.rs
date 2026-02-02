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
//! use lumos::prelude::*;
//!
//! // Load an astronomical image
//! let image = AstroImage::from_file("light_001.fits")?;
//!
//! // Detect stars
//! let config = StarDetectionConfig::default();
//! let result = find_stars(&image, &config);
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
    BackgroundConfig,
    // Advanced: background estimation
    BackgroundMap,
    // Centroid methods
    CentroidMethod,
    // Sensor defects
    DefectMap,
    // Sub-configs
    FilteringConfig,
    // Advanced: profile fitting
    GaussianFitConfig,
    GaussianFitResult,
    LocalBackgroundMethod,
    MoffatFitConfig,
    MoffatFitResult,
    NoiseModel,
    PsfConfig,
    // Main API
    Star,
    StarDetectionConfig,
    StarDetectionDiagnostics,
    StarDetectionResult,
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
    // Astrometry (plate solving)
    CatalogError,
    CatalogSource,
    CatalogStar,
    // Distortion correction
    DistortionMap,
    FieldCurvature,
    FieldCurvatureConfig,
    FullPhaseCorrelator,
    FullPhaseResult,
    // Interpolation
    InterpolationMethod,
    LogPolarCorrelator,
    LogPolarResult,
    MultiScaleConfig,
    MultiScaleRegistrator,
    PhaseCorrelationConfig,
    // Phase correlation
    PhaseCorrelator,
    PixelSkyMatch,
    PlateSolution,
    PlateSolver,
    PlateSolverConfig,
    QuadHash,
    QuadHasher,
    QuadrantConsistency,
    // Quality assessment
    QualityMetrics,
    RadialDistortion,
    RadialDistortionConfig,
    // RANSAC
    RansacConfig,
    RansacEstimator,
    RansacFailureReason,
    RansacResult,
    // Core types
    RegistrationConfig,
    RegistrationError,
    RegistrationResult,
    // High-level API
    Registrator,
    ResidualStats,
    SolveError,
    StarMatch,
    TangentialDistortion,
    TangentialDistortionConfig,
    ThinPlateSpline,
    TpsConfig,
    TransformMatrix,
    TransformType,
    // Triangle matching
    TriangleMatchConfig,
    WarpConfig,
    Wcs,
    WcsBuilder,
    check_quadrant_consistency,
    estimate_overlap,
    match_triangles,
    quick_register,
    quick_register_stars,
    warp_image,
    warp_to_reference_image,
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
