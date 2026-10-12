//! [`XTransImage`]: an X-Trans frame as the demosaic reads it.

use crate::image::cfa::xtrans_pattern::XTransPattern;
use crate::math::size2us::Size2us;

/// An X-Trans frame the demosaic reads: its samples row by row, its size, and its pattern
/// anchored at its first pixel.
#[derive(Debug)]
pub(crate) struct XTransImage<'a> {
    pub(crate) data: &'a [f32],
    pub(crate) size: Size2us,
    pub(crate) pattern: XTransPattern,
    /// The gains, red, green and blue, that balance the colours for the direction decisions —
    /// see [`tiled`](crate::image::cfa::demosaic::tiled); one each for samples already balanced.
    pub(crate) gains: [f32; 3],
}

impl<'a> XTransImage<'a> {
    /// # Panics
    /// When `data` does not hold one sample per pixel of `size`, or `size` is empty.
    pub(crate) fn new(data: &'a [f32], size: Size2us, pattern: XTransPattern) -> Self {
        assert!(
            size.width > 0 && size.height > 0 && data.len() == size.pixel_count(),
            "an X-Trans frame of {}x{} holds {} samples",
            size.width,
            size.height,
            data.len()
        );
        debug_assert!(
            data.iter().all(|v| v.is_finite()),
            "XTransImage data contains NaN or Infinity values"
        );
        Self {
            data,
            size,
            pattern,
            gains: [1.0; 3],
        }
    }

    /// This frame, its colours balanced by `gains` for the direction decisions.
    pub(crate) const fn with_gains(self, gains: [f32; 3]) -> Self {
        Self { gains, ..self }
    }

    /// The sample at `(x, y)`.
    #[inline(always)]
    pub(crate) const fn read(&self, y: usize, x: usize) -> f32 {
        self.data[y * self.size.width + x]
    }
}
