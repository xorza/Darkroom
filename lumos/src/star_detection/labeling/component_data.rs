//! [`ComponentData`]: where one labelled component sits and how large it is.

use crate::math::urect::URect;

/// Where one connected component sits and how many pixels it holds, collected from its runs as
/// the labeling gathers them. The runs themselves stay in the label map.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ComponentData {
    /// Bounding box of the component.
    pub(crate) bbox: URect,
    /// Component label: `i + 1` for the label map's component `i`.
    pub(crate) label: u32,
    /// Number of pixels in the component.
    pub(crate) area: usize,
}
