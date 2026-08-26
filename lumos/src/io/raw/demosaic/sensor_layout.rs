//! Where a demosaicked window sits inside the buffer it was read from.

use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;

/// The visible window inside a raw frame: where it starts, how big it is, and the
/// stride of the buffer it sits in.
///
/// The reader carries one per frame and every demosaic entry point takes it whole —
/// the three are only ever meaningful together, and [`Self::validate`] checks them
/// as a set against the buffer they describe.
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
    /// A layout for a buffer already cropped to its visible area: raw and active
    /// extents coincide and there is no margin to skip.
    pub(crate) fn cropped(size: Size2us) -> Self {
        Self {
            raw: size,
            active: size,
            margin: Vec2us::ZERO,
        }
    }

    /// # Panics
    /// Panics if:
    /// - `data_len != raw.pixel_count()`
    /// - `margin.y + active.height > raw.height`
    /// - `margin.x + active.width > raw.width`
    /// - either extent is zero
    pub(crate) fn validate(self, data_len: usize) {
        let Self {
            raw,
            active,
            margin,
        } = self;
        assert!(
            active.width > 0 && active.height > 0,
            "Output dimensions must be non-zero: {}x{}",
            active.width,
            active.height
        );
        assert!(
            raw.width > 0 && raw.height > 0,
            "Raw dimensions must be non-zero: {}x{}",
            raw.width,
            raw.height
        );
        assert!(
            data_len == raw.pixel_count(),
            "Data length {} doesn't match raw dimensions {}x{}={}",
            data_len,
            raw.width,
            raw.height,
            raw.pixel_count()
        );
        assert!(
            margin.y + active.height <= raw.height,
            "Top margin {} + height {} exceeds raw height {}",
            margin.y,
            active.height,
            raw.height
        );
        assert!(
            margin.x + active.width <= raw.width,
            "Left margin {} + width {} exceeds raw width {}",
            margin.x,
            active.width,
            raw.width
        );
    }
}
