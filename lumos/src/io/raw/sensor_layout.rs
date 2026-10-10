//! Where a RAW file's visible window sits inside LibRaw's raw buffer.

use libraw_sys as sys;

use crate::io::raw::error::RawError;
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

impl SensorLayout {
    /// The layout LibRaw's `sizes` state, settled by the open.
    ///
    /// # Errors
    ///
    /// [`RawError::Geometry`] when either area is empty or the visible one does not lie inside
    /// the raw one.
    pub(crate) fn of(sizes: &sys::libraw_image_sizes_t) -> Result<Self, RawError> {
        let raw = Size2us::new(usize::from(sizes.raw_width), usize::from(sizes.raw_height));
        let active = Size2us::new(usize::from(sizes.width), usize::from(sizes.height));
        let margin = Vec2us::new(
            usize::from(sizes.left_margin),
            usize::from(sizes.top_margin),
        );
        if raw.pixel_count() == 0
            || active.pixel_count() == 0
            || margin.x + active.width > raw.width
            || margin.y + active.height > raw.height
        {
            return Err(RawError::Geometry {
                raw,
                active,
                margin,
            });
        }
        Ok(Self {
            raw,
            active,
            margin,
        })
    }
}
