//! Image registration module for astronomical image alignment.
//!
//! This module provides state-of-the-art image registration algorithms optimized
//! for astrophotography, including:
//!
//! - **Triangle matching**: Robust star pattern matching (Siril/Astroalign approach)
//! - **Phase correlation**: FFT-based coarse alignment for large offsets
//! - **RANSAC**: Robust transformation estimation with outlier rejection
//! - **Lanczos interpolation**: High-quality sub-pixel image resampling
//!
//! # Transformation Models
//!
//! Supported transformation types with increasing degrees of freedom:
//!
//! | Type | DOF | Description |
//! |------|-----|-------------|
//! | Translation | 2 | X/Y offset only |
//! | Euclidean | 3 | Translation + rotation |
//! | Similarity | 4 | Translation + rotation + uniform scale |
//! | Affine | 6 | Handles shear and differential scaling |
//! | Homography | 8 | Full perspective transformation |
//!
//! # Example
//!
//! ```ignore
//! use lumos::registration::{Registrator, RegistrationConfig, TransformType};
//!
//! // Prepare star positions (x, y) from both images
//! let ref_stars = vec![(100.0, 100.0), (200.0, 150.0), /* ... */];
//! let target_stars = vec![(110.0, 105.0), (210.0, 155.0), /* ... */];
//!
//! // Configure registration
//! let config = RegistrationConfig {
//!     transform_type: TransformType::Similarity,
//!     ..Default::default()
//! };
//!
//! // Run registration
//! let registrator = Registrator::new(config);
//! let result = registrator.register_stars(&ref_stars, &target_stars)?;
//!
//! println!("RMS error: {:.3} pixels", result.rms_error);
//! println!("Matched {} stars", result.num_inliers);
//!
//! // Apply transformation to align target image
//! let aligned = warp_to_reference_image(&target_image, &result.transform, InterpolationMethod::Lanczos3);
//! ```
//!
//! # Algorithm Overview
//!
//! The registration pipeline consists of:
//!
//! 1. **Star Detection**: Uses existing `star_detection` module
//! 2. **Coarse Alignment** (optional): Phase correlation for large offsets
//! 3. **Triangle Matching**: Geometric hashing for star pattern matching
//! 4. **RANSAC**: Robust transformation estimation
//! 5. **Refinement**: Least-squares optimization on inliers
//! 6. **Warping**: High-quality image resampling
//!
//! See `IMPLEMENTATION_PLAN.md` for detailed algorithm documentation.

pub(crate) mod astrometry;
pub(crate) mod config;
pub(crate) mod distortion;
pub(crate) mod interpolation;
pub(crate) mod phase_correlation;
pub(crate) mod pipeline;
pub(crate) mod quality;
pub(crate) mod ransac;
pub(crate) mod spatial;
pub(crate) mod transform;
pub(crate) mod triangle;

#[cfg(test)]
mod tests;

// Re-export all configuration types from the consolidated config module
pub use config::{
    InterpolationMethod, MultiScaleConfig, PhaseCorrelationConfig, RansacConfig,
    RegistrationConfig, SipCorrectionConfig, SubpixelMethod, TriangleMatchConfig, WarpConfig,
};

// High-level pipeline API (primary entry point)
pub use pipeline::{
    MultiScaleRegistrator, RansacFailureReason, RegistrationError, RegistrationResult, Registrator,
    quick_register, quick_register_stars, warp_to_reference_image,
};

// Core types needed by users
pub use transform::{Transform, TransformType};
pub use triangle::StarMatch;

// Distortion types
pub use distortion::{
    DistortionMap, FieldCurvature, FieldCurvatureConfig, RadialDistortion, RadialDistortionConfig,
    SipConfig, SipPolynomial, TangentialDistortion, TangentialDistortionConfig, ThinPlateSpline,
    TpsConfig,
};

// Interpolation (non-config types)
pub use interpolation::warp_image;

// Phase correlation (non-config types)
pub use phase_correlation::{
    FullPhaseCorrelator, FullPhaseResult, LogPolarCorrelator, LogPolarResult, PhaseCorrelator,
};

// RANSAC (non-config types)
pub use ransac::{RansacEstimator, RansacResult};

// Quality assessment
pub use quality::{
    QuadrantConsistency, QualityMetrics, ResidualStats, check_quadrant_consistency,
    estimate_overlap,
};

// Triangle matching (non-config types)
pub use triangle::match_triangles;

// Astrometry (plate solving)
pub use astrometry::{
    CatalogError, CatalogSource, CatalogStar, PixelSkyMatch, PlateSolution, PlateSolver,
    PlateSolverConfig, QuadHash, QuadHasher, SolveError, Wcs, WcsBuilder,
};
