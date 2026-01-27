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
//! let config = RegistrationConfig::builder()
//!     .with_scale()
//!     .ransac_threshold(2.0)
//!     .build();
//!
//! // Run registration
//! let registrator = Registrator::new(config);
//! let result = registrator.register_stars(&ref_stars, &target_stars)?;
//!
//! println!("RMS error: {:.3} pixels", result.rms_error);
//! println!("Matched {} stars", result.num_inliers);
//!
//! // Apply transformation to align target image
//! let aligned = warp_to_reference(&target_image, width, height, &result.transform, InterpolationMethod::Lanczos3);
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

pub(crate) mod constants;
pub(crate) mod distortion;
pub(crate) mod gpu;
pub(crate) mod interpolation;
pub(crate) mod phase_correlation;
pub(crate) mod pipeline;
pub(crate) mod quality;
pub(crate) mod ransac;
pub(crate) mod spatial;
pub(crate) mod triangle;
pub(crate) mod types;

#[cfg(test)]
mod tests;

#[cfg(feature = "bench")]
pub mod bench {
    //! Benchmark module for registration operations.

    use criterion::Criterion;

    /// Register all registration benchmarks with Criterion.
    pub fn benchmarks(c: &mut Criterion) {
        super::types::bench::benchmarks(c);
        super::triangle::bench::benchmarks(c);
        super::ransac::bench::benchmarks(c);
        super::phase_correlation::bench::benchmarks(c);
        super::interpolation::bench::benchmarks(c);
        super::pipeline::bench::benchmarks(c);
        super::quality::bench::benchmarks(c);
    }
}

// Re-export main public API types
// High-level pipeline API (primary entry point)
pub use pipeline::{
    MultiScaleConfig, MultiScaleRegistrator, Registrator, quick_register, register_stars,
    warp_to_reference,
};

// Core types needed by users
pub use types::{
    RansacFailureReason, RegistrationConfig, RegistrationConfigBuilder, RegistrationError,
    RegistrationResult, StarMatch, TransformMatrix, TransformType,
};

// GPU-accelerated warping
pub use gpu::GpuWarper;

// Configuration types
pub use distortion::{DistortionMap, ThinPlateSpline, TpsConfig};
pub use interpolation::{InterpolationMethod, WarpConfig, warp_image};
pub use phase_correlation::{
    FullPhaseCorrelator, FullPhaseResult, LogPolarCorrelator, LogPolarResult,
    PhaseCorrelationConfig, PhaseCorrelator,
};
pub use ransac::{RansacConfig, RansacEstimator, RansacResult};

// Quality assessment
pub use quality::{
    QuadrantConsistency, QualityMetrics, ResidualStats, check_quadrant_consistency,
    estimate_overlap,
};

// Triangle matching
pub use triangle::{TriangleMatchConfig, match_triangles};
