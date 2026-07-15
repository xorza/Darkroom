//! Triangle matching for point pattern recognition.
//!
//! This module implements triangle-based point pattern matching using invariant
//! side ratios indexed in a k-d tree. Triangles are characterized by their
//! side ratios, which are invariant to translation, rotation, and scale.
//!
//! # Algorithm Overview
//!
//! 1. Form triangles from positions using k-nearest neighbors (k-d tree)
//! 2. Compute scale-invariant descriptors (sorted side ratios)
//! 3. Index reference triangles in a k-d tree on their invariant ratios
//! 4. For each target triangle, find similar reference triangles by radius search
//! 5. Vote for point correspondences based on shared vertices
//! 6. Extract high-confidence matches from vote matrix

mod geometry;
pub(crate) mod matching;
#[cfg(test)]
mod tests;
pub(crate) mod voting;

/// Configuration for triangle matching.
#[derive(Debug, Clone)]
pub struct TriangleConfig {
    pub ratio_tolerance: f64,
    pub min_votes: usize,
    pub check_orientation: bool,
}

impl Default for TriangleConfig {
    fn default() -> Self {
        Self {
            ratio_tolerance: 0.01,
            min_votes: 3,
            check_orientation: true,
        }
    }
}
