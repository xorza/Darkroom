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
//! use lumos::registration::{register_images, RegistrationConfig};
//!
//! let config = RegistrationConfig::builder()
//!     .with_rotation()
//!     .with_scale()
//!     .ransac_threshold(2.0)
//!     .build();
//!
//! let result = register_images(&reference, &target, &config)?;
//! println!("RMS error: {:.3} pixels", result.rms_error);
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

// TODO: Implement modules as per IMPLEMENTATION_PLAN.md
//
// Phase 1: Core types
// pub mod types;
//
// Phase 2: Triangle matching
// pub mod star_matching;
//
// Phase 3: RANSAC
// pub mod transform;
//
// Phase 4: Phase correlation
// pub mod phase_correlation;
//
// Phase 5: Interpolation
// pub mod interpolation;
//
// Phase 6: Image warping
// pub mod warp;
//
// Phase 7: Quality metrics
// pub mod quality;
//
// Benchmarks (feature-gated)
// #[cfg(feature = "bench")]
// pub mod bench;
