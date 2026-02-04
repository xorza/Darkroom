//! Triangle matching for point pattern recognition.
//!
//! This module implements geometric hashing based on triangles formed from 2D points.
//! Triangles are characterized by their side ratios, which are invariant to
//! translation, rotation, and scale.
//!
//! # Algorithm Overview
//!
//! 1. Form triangles from positions using k-nearest neighbors (k-d tree)
//! 2. Compute scale-invariant descriptors (sorted side ratios)
//! 3. Hash reference triangles into bins by their ratios
//! 4. For each target triangle, lookup matching reference triangles
//! 5. Vote for point correspondences based on shared vertices
//! 6. Extract high-confidence matches from vote matrix
//! 7. Optionally refine with transform-guided two-step matching

mod geometry;
mod hash_table;
mod matching;
#[cfg(test)]
mod tests;
mod voting;

pub use matching::{form_triangles_kdtree, match_triangles};
pub use voting::PointMatch;

pub use crate::registration::config::TriangleMatchConfig;
