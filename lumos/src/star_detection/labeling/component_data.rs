//! [`ComponentData`]: where one labelled component sits and how large it is.

use common::USizeRect2;

/// Where one connected component sits and how many pixels it holds; its runs stay in the label
/// map.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ComponentData {
    pub(crate) bbox: USizeRect2,
    /// Component label: `i + 1` for the label map's component `i`.
    pub(crate) label: u32,
    pub(crate) area: usize,
}
