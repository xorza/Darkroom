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

/// Threshold for switching between dense and sparse vote matrix storage.
///
/// When `n_ref * n_target < DENSE_VOTE_THRESHOLD`, use a dense Vec<u16> matrix.
/// Otherwise, use a sparse HashMap for memory efficiency.
///
/// Memory analysis at threshold (250,000 entries):
/// - Dense: 250,000 * 2 bytes (u16) = 500 KB
/// - Sparse: Only stores non-zero votes, but each entry costs ~40 bytes
///   (key: 16 bytes + value: 8 bytes + HashMap overhead)
///
/// Dense is faster for small star counts due to direct indexing (O(1) vs hash lookup).
/// For 500x500 stars (250K entries), dense is still preferred. Beyond that, sparse wins.
pub const DENSE_VOTE_THRESHOLD: usize = 250_000;

// =============================================================================
// Triangle matching defaults
// =============================================================================

/// Default tolerance for triangle side ratio comparison (1% = 0.01).
///
/// Two triangles match if their sorted side ratios differ by less than this tolerance.
/// Tighter tolerance = fewer false matches but may miss true matches with noise.
/// Looser tolerance = more matches but higher false positive rate.
pub const DEFAULT_TRIANGLE_TOLERANCE: f64 = 0.01;

/// Default number of hash table bins per dimension for geometric hashing.
///
/// Triangles are binned by their two side ratios into a 2D grid.
/// 100 bins per dimension = 10,000 total buckets. This balances:
/// - Too few bins: Many triangles per bucket, slow lookup
/// - Too many bins: Most buckets empty, wasted memory, boundary effects
pub const DEFAULT_HASH_BINS: usize = 100;

/// Default minimum votes required to accept a star correspondence.
///
/// A vote is cast when triangles from ref/target share a vertex correspondence.
/// Higher threshold = more confident matches but fewer total matches.
pub const DEFAULT_MIN_VOTES: usize = 3;

/// Default maximum number of stars to use for triangle matching.
///
/// Limits computational cost: O(n²) for kdtree triangle formation.
/// Stars should be sorted by brightness; we take the brightest N.
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
