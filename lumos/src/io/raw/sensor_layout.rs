//! Where a RAW file's visible window sits inside LibRaw's raw buffer.

use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;

/// The visible window inside a raw frame: where it starts, how big it is, and the
/// stride of the buffer it sits in. The reader crops to the window as it normalizes, so nothing
/// downstream sees the masked margins.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SensorLayout {
    /// Extent of the source buffer, which spans the masked margins as well as `active`.
    pub(crate) raw: Size2us,
    /// Extent of the visible window.
    pub(crate) active: Size2us,
    /// Top-left corner of the window within the source buffer.
    pub(crate) margin: Vec2us,
}
