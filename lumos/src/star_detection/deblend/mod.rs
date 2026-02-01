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

use crate::common::Buffer2;
use crate::math::{Aabb, Vec2us};
use crate::star_detection::detection::LabelMap;

pub mod local_maxima;
pub mod multi_threshold;

#[cfg(test)]
mod integration_tests;

// Re-export config types used by submodules
pub use super::config::DeblendConfig;
pub use local_maxima::deblend_local_maxima;
pub use multi_threshold::deblend_multi_threshold;

// ============================================================================
// Constants
// ============================================================================

/// Maximum number of peaks/candidates per component.
/// Components with more peaks than this will have excess peaks ignored.
pub const MAX_PEAKS: usize = 8;

// ============================================================================
// Types
// ============================================================================

/// A pixel with its coordinates and value.
#[derive(Debug, Clone, Copy)]
pub struct Pixel {
    pub pos: Vec2us,
    pub value: f32,
}

/// Result of deblending a single connected component.
#[derive(Debug, Clone)]
pub struct DeblendedCandidate {
    pub bbox: Aabb,
    pub peak: Vec2us,
    pub peak_value: f32,
    pub area: usize,
}

/// Data for a connected component (allocation-free).
///
/// Instead of storing pixel coordinates, we store the component label
/// and iterate over the bounding box on-demand, checking the labels buffer.
#[derive(Debug, Clone, Copy)]
pub struct ComponentData {
    /// Bounding box of the component.
    pub bbox: Aabb,
    /// Component label in the labels buffer.
    pub label: u32,
    /// Number of pixels in the component (pre-computed).
    pub area: usize,
}

impl ComponentData {
    /// Iterate over all pixels in this component.
    ///
    /// Scans the bounding box and yields pixels that match the component label.
    #[inline]
    pub fn iter_pixels<'a>(
        &'a self,
        pixels: &'a Buffer2<f32>,
        labels: &'a LabelMap,
    ) -> impl Iterator<Item = Pixel> + 'a {
        let width = pixels.width();
        let bbox = &self.bbox;
        (bbox.min.y..=bbox.max.y).flat_map(move |y| {
            (bbox.min.x..=bbox.max.x).filter_map(move |x| {
                let idx = y * width + x;
                if labels[idx] == self.label {
                    Some(Pixel {
                        pos: Vec2us::new(x, y),
                        value: pixels[idx],
                    })
                } else {
                    None
                }
            })
        })
    }

    /// Find the peak pixel (maximum value) in this component.
    #[inline]
    pub fn find_peak(&self, pixels: &Buffer2<f32>, labels: &LabelMap) -> Pixel {
        self.iter_pixels(pixels, labels)
            .max_by(|a, b| {
                a.value
                    .partial_cmp(&b.value)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap_or(Pixel {
                pos: self.bbox.min,
                value: 0.0,
            })
    }
}
