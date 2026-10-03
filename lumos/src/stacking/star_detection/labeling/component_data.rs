//! [`ComponentData`]: where one labelled component sits and how large it is.

use crate::math::urect::URect;

/// Where one connected component sits and how many pixels it holds, collected from its runs as
/// the labeling writes them. The pixels themselves stay in the label map, read back through the
/// bounding box.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ComponentData {
    /// Bounding box of the component.
    pub(crate) bbox: URect,
    /// Component label in the labels buffer.
    pub(crate) label: u32,
    /// Number of pixels in the component.
    pub(crate) area: usize,
}
