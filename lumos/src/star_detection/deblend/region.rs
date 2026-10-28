//! Connected region from segmentation and deblending.

use common::USizeRect2;
use glam::USizeVec2;

/// A connected region of pixels identified during detection.
///
/// Represents a candidate source region after thresholding, connected component
/// labeling, and optional deblending. Each region may correspond to a single
/// star or other source.
#[derive(Debug)]
pub(crate) struct Region {
    /// Bounding box of the region.
    pub(crate) bbox: USizeRect2,
    /// Peak pixel coordinates within the region.
    pub(crate) peak: USizeVec2,
    /// Number of pixels in the region.
    pub(crate) area: usize,
}
