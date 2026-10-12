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
pub(super) mod matching;
pub(crate) mod triangle_config;
pub(crate) mod voting;

#[cfg(feature = "bench")]
pub(crate) mod bench;
#[cfg(test)]
mod tests;
