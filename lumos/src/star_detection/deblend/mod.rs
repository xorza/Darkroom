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
use crate::star_detection::detection::LabelMap;

pub mod local_maxima;
pub mod multi_threshold;

#[cfg(test)]
mod tests;

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
    pub x: usize,
    pub y: usize,
    pub value: f32,
}

/// Data for a connected component (allocation-free).
///
/// Instead of storing pixel coordinates, we store the component label
/// and iterate over the bounding box on-demand, checking the labels buffer.
#[derive(Debug, Clone, Copy)]
pub struct ComponentData {
    pub x_min: usize,
    pub x_max: usize,
    pub y_min: usize,
    pub y_max: usize,
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
        (self.y_min..=self.y_max).flat_map(move |y| {
            (self.x_min..=self.x_max).filter_map(move |x| {
                let idx = y * width + x;
                if labels[idx] == self.label {
                    Some(Pixel {
                        x,
                        y,
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
                x: self.x_min,
                y: self.y_min,
                value: 0.0,
            })
    }
}

// ============================================================================
// Re-exports
// ============================================================================

pub use local_maxima::deblend_local_maxima;
#[allow(unused_imports)]
pub use local_maxima::{DeblendedCandidate, deblend_by_nearest_peak, find_local_maxima};
#[allow(unused_imports)]
pub use multi_threshold::DeblendedObject;
pub use multi_threshold::{MultiThresholdDeblendConfig, deblend_component};

// Re-export DeblendConfig from config module
pub use super::config::DeblendConfig;
