//! Connected region from segmentation and deblending.

use common::USizeRect2;
use glam::USizeVec2;

/// A candidate source after thresholding, labeling and optional deblending.
#[derive(Debug)]
pub(crate) struct Region {
    pub(crate) bbox: USizeRect2,
    pub(crate) peak: USizeVec2,
    pub(crate) area: usize,
}
