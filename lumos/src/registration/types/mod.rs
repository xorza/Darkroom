//! Core types for image registration.

#[cfg(test)]
mod tests;

/// A matched star pair between reference and target.
#[derive(Debug, Clone, Copy)]
pub struct StarMatch {
    /// Index in reference star list.
    pub ref_idx: usize,
    /// Index in target star list.
    pub target_idx: usize,
    /// Number of votes from triangle matching.
    pub votes: usize,
    /// Match confidence (0.0 - 1.0).
    pub confidence: f64,
}
