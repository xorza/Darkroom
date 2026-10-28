//! [`SensorLayout`]: where a RAW file's visible window sits inside LibRaw's raw buffer.

use common::USize2;
use common::glam::USizeVec2;

use crate::error::LibrawError;
use crate::sys;

/// The visible window inside a raw frame: where it starts, how big it is, and the extent of the
/// buffer it sits in, which spans the masked margins as well.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SensorLayout {
    /// Extent of the source buffer, which spans the masked margins as well as `active`.
    pub raw: USize2,
    /// Extent of the visible window.
    pub active: USize2,
    /// Top-left corner of the window within the source buffer.
    pub margin: USizeVec2,
}

impl SensorLayout {
    /// The layout LibRaw's `sizes` state, settled by the open.
    ///
    /// # Errors
    ///
    /// [`LibrawError::Geometry`] when either area is empty or the visible one does not lie inside
    /// the raw one.
    pub(crate) fn of(sizes: &sys::libraw_image_sizes_t) -> Result<Self, LibrawError> {
        let raw = USize2::new(usize::from(sizes.raw_width), usize::from(sizes.raw_height));
        let active = USize2::new(usize::from(sizes.width), usize::from(sizes.height));
        let margin = USizeVec2::new(
            usize::from(sizes.left_margin),
            usize::from(sizes.top_margin),
        );
        if raw.pixel_count() == 0
            || active.pixel_count() == 0
            || margin.x + active.width > raw.width
            || margin.y + active.height > raw.height
        {
            return Err(LibrawError::Geometry {
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
