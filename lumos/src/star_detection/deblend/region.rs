//! Connected region from segmentation and deblending.

use crate::math::urect::URect;
use crate::math::vec2us::Vec2us;

/// A connected region of pixels identified during detection.
///
/// Represents a candidate source region after thresholding, connected component
/// labeling, and optional deblending. Each region may correspond to a single
/// star or other source.
#[derive(Debug)]
pub(crate) struct Region {
    /// Bounding box of the region.
    pub(crate) bbox: URect,
    /// Peak pixel coordinates within the region.
    pub(crate) peak: Vec2us,
    /// Number of pixels in the region.
    pub(crate) area: usize,
}
