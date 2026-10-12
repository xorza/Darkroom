//! [`BayerImage`]: a Bayer frame as the demosaic reads it.

use crate::image::cfa::cfa_pattern::CfaPattern;
use crate::math::size2us::Size2us;

/// A Bayer frame the demosaic reads: its samples row by row — calibration may put them outside
/// `[0, 1]` — its size, and its pattern anchored at its first pixel.
#[derive(Debug)]
pub(crate) struct BayerImage<'a> {
    pub(crate) data: &'a [f32],
    pub(crate) size: Size2us,
    pub(crate) pattern: CfaPattern,
    /// The gains, red, green and blue, that balance the colours for the direction decisions —
    /// see [`tiled`](crate::image::cfa::demosaic::tiled); one each for samples already balanced.
    pub(crate) gains: [f32; 3],
}

impl<'a> BayerImage<'a> {
    /// # Panics
    /// When `data` does not hold one sample per pixel of `size`, or `size` is empty.
    pub(crate) fn new(data: &'a [f32], size: Size2us, pattern: CfaPattern) -> Self {
        assert!(
            size.width > 0 && size.height > 0 && data.len() == size.pixel_count(),
            "a Bayer frame of {}x{} holds {} samples",
            size.width,
            size.height,
            data.len()
        );
        debug_assert!(
            data.iter().all(|v| v.is_finite()),
            "BayerImage data contains NaN or Infinity values"
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
}
