//! Connected region from segmentation and deblending.

use crate::math::{Aabb, Vec2us};

/// A connected region of pixels identified during detection.
///
/// Represents a candidate source region after thresholding, connected component
/// labeling, and optional deblending. Each region may correspond to a single
/// star or other source.
#[derive(Debug)]
pub struct Region {
    /// Bounding box of the region.
    #[allow(dead_code)] // Used by candidate_detection, will be used in measure stage
    pub bbox: Aabb,
    /// Peak pixel coordinates within the region.
    pub peak: Vec2us,
    /// Peak pixel value.
    pub peak_value: f32,
    /// Number of pixels in the region.
    #[allow(dead_code)] // Used by candidate_detection, will be used in measure stage
    pub area: usize,
}
