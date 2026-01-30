//! Star deblending algorithms for separating overlapping sources.
//!
//! This module provides two deblending approaches:
//!
//! 1. **Local maxima deblending** (`local_maxima`): Fast algorithm that finds
//!    peaks in a connected component and assigns pixels to the nearest peak.
//!    Good for well-separated stars.
//!
//! 2. **Multi-threshold deblending** (`multi_threshold`): SExtractor-style
//!    tree-based algorithm that uses multiple threshold levels to separate
//!    blended sources. More accurate for crowded fields but slower.

pub mod local_maxima;
pub mod multi_threshold;

#[cfg(test)]
mod tests;

// Re-export main types and functions
pub use local_maxima::{ComponentData, Pixel, deblend_local_maxima};
#[allow(unused_imports)]
pub use local_maxima::{DeblendedCandidate, deblend_by_nearest_peak, find_local_maxima};
#[allow(unused_imports)]
pub use multi_threshold::DeblendedObject;
pub use multi_threshold::{MultiThresholdDeblendConfig, deblend_component};

/// Configuration for deblending algorithm.
#[derive(Debug, Clone, Copy)]
pub struct DeblendConfig {
    /// Minimum separation between peaks for deblending (in pixels).
    pub min_separation: usize,
    /// Minimum peak prominence as fraction of primary peak for deblending.
    pub min_prominence: f32,
    /// Enable multi-threshold deblending (SExtractor-style).
    pub multi_threshold: bool,
    /// Number of sub-thresholds for multi-threshold deblending.
    pub n_thresholds: usize,
    /// Minimum contrast for multi-threshold deblending.
    pub min_contrast: f32,
}

impl Default for DeblendConfig {
    fn default() -> Self {
        Self {
            min_separation: 3,
            min_prominence: 0.3,
            multi_threshold: false,
            n_thresholds: 32,
            min_contrast: 0.005,
        }
    }
}
