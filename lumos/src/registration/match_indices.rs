//! [`MatchIndices`]: a reference star paired with a target star.

/// A reference star paired with a target star, by index into their respective slices.
///
/// The bare pair, shared by everything that carries one: `PointMatch` adds the vote evidence
/// that produced it, and `StarMatch` adds the residual only measurable once a transform exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatchIndices {
    /// Index into the reference star slice.
    pub reference: usize,
    /// Index into the target star slice.
    pub target: usize,
}
