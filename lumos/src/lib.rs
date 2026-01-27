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

#[cfg(any(test, feature = "bench"))]
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
    // Advanced: background estimation
    BackgroundMap,
    // Centroid methods
    CentroidMethod,
    // Sensor defects
    DefectMap,
    // Advanced: profile fitting
    GaussianFitConfig,
    GaussianFitResult,
    IterativeBackgroundConfig,
    LocalBackgroundMethod,
    MoffatFitConfig,
    MoffatFitResult,
    // Main API
    Star,
    StarDetectionConfig,
    StarDetectionConfigBuilder,
    StarDetectionDiagnostics,
    StarDetectionResult,
    alpha_beta_to_fwhm,
    estimate_background,
    estimate_background_image,
    estimate_background_iterative,
    estimate_background_iterative_image,
    find_stars,
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
    // GPU warping
    GpuWarper,
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
    RegistrationConfigBuilder,
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
    register_star_positions,
    warp_image,
    warp_to_reference,
    warp_to_reference_image,
};
// Re-export deprecated alias for backwards compatibility
#[allow(deprecated)]
pub use registration::register_stars;

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
    // GPU stacking
    GpuSigmaClipConfig,
    GpuSigmaClipPipeline,
    GpuSigmaClipper,
    // Gradient removal
    GradientModel,
    GradientRemovalConfig,
    GradientRemovalError,
    GradientRemovalResult,
    // Main API
    ImageStack,
    // Live stacking
    LiveFrameQuality,
    LiveQualityStream,
    LiveStackAccumulator,
    LiveStackConfig,
    LiveStackConfigBuilder,
    LiveStackError,
    LiveStackMode,
    LiveStackResult,
    LiveStackStats,
    MAX_GPU_FRAMES,
    MedianConfig,
    // Multi-session stacking
    MultiSessionStack,
    MultiSessionSummary,
    ObjectPosition,
    // Progress reporting
    ProgressCallback,
    Session,
    SessionConfig,
    SessionId,
    SessionQuality,
    SessionSummary,
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

// ============================================================================
// Benchmark support (feature = "bench")
// ============================================================================

#[cfg(feature = "bench")]
mod bench_impl;

/// Benchmark proxies for internal functions.
///
/// This module exposes internal implementation details for benchmarking purposes.
/// These APIs are unstable and not meant for production use.
#[cfg(feature = "bench")]
pub mod bench {
    // Demosaicing benchmarks
    pub use crate::astro_image::demosaic::bench as demosaic;
    pub use crate::astro_image::hot_pixels::bench as hot_pixels;

    // Pipeline benchmarks
    pub use crate::bench_impl::pipeline;

    // Math benchmarks
    pub use crate::math::bench as math;

    // Stacking benchmarks
    pub use crate::stacking::bench::{gpu, mean, median, sigma_clipped};

    // Star detection benchmarks
    pub use crate::star_detection::bench::{
        background, centroid, convolution, cosmic_ray, deblend, detection, median_filter,
    };

    // Testing utilities for benchmarks
    pub use crate::testing::{calibration_dir, calibration_masters_dir, first_raw_file};

    // Re-export registration bench module
    pub use crate::registration::bench as registration;

    // Re-export internal types needed by some benchmarks
    pub use crate::star_detection::background::BackgroundMap;
    pub use crate::star_detection::detection::{create_threshold_mask, scalar as detection_scalar};
}
