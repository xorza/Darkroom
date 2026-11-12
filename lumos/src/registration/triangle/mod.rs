//! Triangle matching for point pattern recognition.
//!
//! A triangle's side ratios survive translation, rotation and scale, so triangles of two frames are
//! matched by their ratios, found by radius search in a k-d tree; each similar pair votes for the
//! point correspondences its vertices imply, and the pairs with enough votes are the matches.

mod geometry;
mod matching;
mod reference_triangles;
pub(crate) mod triangle_config;
pub(crate) mod voting;

#[cfg(feature = "internals")]
pub(crate) mod bench;
#[cfg(test)]
mod tests;
