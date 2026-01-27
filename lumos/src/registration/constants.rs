//! Shared constants for the registration module.
//!
//! This module centralizes magic numbers and algorithm defaults to improve
//! maintainability and make tuning easier.

// =============================================================================
// Triangle matching settings
// =============================================================================

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
