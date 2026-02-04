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
//! 7. Optionally refine with transform-guided two-step matching

mod geometry;
mod matching;
#[cfg(test)]
mod tests;
mod voting;

pub use matching::{form_triangles_kdtree, match_triangles};
pub use voting::PointMatch;

pub use crate::registration::config::TriangleMatchConfig;
