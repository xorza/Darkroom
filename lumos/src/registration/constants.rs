//! Shared constants for the registration module.
//!
//! This module centralizes magic numbers and algorithm defaults to improve
//! maintainability and make tuning easier.

// =============================================================================
// Numerical precision thresholds
// =============================================================================

/// General purpose epsilon for floating point comparisons.
pub const EPSILON: f64 = 1e-10;

/// Threshold for detecting singular matrices.
pub const SINGULAR_THRESHOLD: f64 = 1e-12;

/// Threshold for detecting collinear points.
pub const COLLINEAR_THRESHOLD: f64 = 1e-15;

/// Minimum side length for valid triangles.
pub const MIN_TRIANGLE_SIDE: f64 = 1e-10;

/// Minimum area squared for valid triangles (Heron's formula).
/// Prevents very flat/degenerate triangles.
pub const MIN_TRIANGLE_AREA_SQ: f64 = 1e-6;

// =============================================================================
// Vote matrix settings
// =============================================================================

/// Threshold for using dense vote matrix (n_ref * n_target < this value).
/// Dense is faster due to direct indexing, but uses more memory (~500KB for u16 matrix at threshold).
pub const DENSE_VOTE_THRESHOLD: usize = 250_000;

// =============================================================================
// Triangle matching defaults
// =============================================================================

/// Default tolerance for side ratio comparison.
pub const DEFAULT_TRIANGLE_TOLERANCE: f64 = 0.01;

/// Default number of hash table bins per dimension.
pub const DEFAULT_HASH_BINS: usize = 100;

/// Default minimum votes required to accept a match.
pub const DEFAULT_MIN_VOTES: usize = 3;

/// Default maximum number of stars to use for matching.
pub const DEFAULT_MAX_STARS: usize = 50;

// =============================================================================
// RANSAC defaults
// =============================================================================

/// Default inlier distance threshold in pixels.
pub const DEFAULT_RANSAC_THRESHOLD: f64 = 2.0;

/// Default maximum RANSAC iterations.
pub const DEFAULT_MAX_RANSAC_ITERATIONS: usize = 1000;

/// Default confidence level for early termination.
pub const DEFAULT_RANSAC_CONFIDENCE: f64 = 0.999;

/// Default minimum inlier ratio to accept model.
pub const DEFAULT_MIN_INLIER_RATIO: f64 = 0.5;

/// Default maximum iterations for local optimization.
pub const DEFAULT_LO_MAX_ITERATIONS: usize = 10;

// =============================================================================
// SIMD dispatch thresholds
// =============================================================================

/// Minimum point count before using AVX2 SIMD.
pub const SIMD_AVX2_MIN_POINTS: usize = 4;

/// Minimum point count before using SSE SIMD.
pub const SIMD_SSE_MIN_POINTS: usize = 2;

/// Minimum point count before using NEON SIMD.
pub const SIMD_NEON_MIN_POINTS: usize = 2;

// =============================================================================
// Quality thresholds
// =============================================================================

/// Minimum number of inliers for valid registration.
pub const MIN_INLIERS_FOR_VALID: usize = 4;

/// Maximum RMS error for valid registration (pixels).
pub const MAX_RMS_ERROR_FOR_VALID: f64 = 5.0;

/// Minimum inlier ratio for valid registration.
pub const MIN_INLIER_RATIO_FOR_VALID: f64 = 0.3;

/// Maximum RMS difference between quadrants for consistency.
pub const MAX_QUADRANT_RMS_DIFFERENCE: f64 = 2.0;
