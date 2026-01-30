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
pub use multi_threshold::{
    MultiThresholdComponentData, MultiThresholdDeblendConfig, deblend_component,
};

// Re-export DeblendConfig from config module
pub use super::config::DeblendConfig;
