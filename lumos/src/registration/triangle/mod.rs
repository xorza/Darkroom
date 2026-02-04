//! Triangle matching for star pattern recognition.
//!
//! This module implements geometric hashing based on triangles formed from stars.
//! Triangles are characterized by their side ratios, which are invariant to
//! translation, rotation, and scale.
//!
//! # Algorithm Overview
//!
//! 1. Form triangles from star positions using k-nearest neighbors (k-d tree)
//! 2. Compute scale-invariant descriptors (sorted side ratios)
//! 3. Hash reference triangles into bins by their ratios
//! 4. For each target triangle, lookup matching reference triangles
//! 5. Vote for star correspondences based on shared vertices
//! 6. Extract high-confidence matches from vote matrix
//! 7. Optionally refine with transform-guided two-step matching

mod geometry;
mod hash_table;
mod matching;
#[cfg(test)]
mod tests;
mod voting;

pub use matching::{form_triangles_kdtree, match_triangles};
pub use voting::StarMatch;

pub use crate::registration::config::TriangleMatchConfig;

// Re-export internal types for use within the registration crate
pub(crate) use geometry::{Orientation, Triangle};
pub(crate) use hash_table::TriangleHashTable;
pub(crate) use matching::{compute_position_threshold, estimate_similarity_transform};
pub(crate) use voting::{VoteMatrix, resolve_matches, vote_for_correspondences};
