//! [`BayerImage`]: a Bayer frame as the demosaic reads it.

use crate::image::cfa::cfa_pattern::CfaPattern;
use crate::math::size2us::Size2us;
use imaginarium::Buffer2;

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
    /// When `data` is empty.
    pub(crate) fn new(data: &'a Buffer2<f32>, pattern: CfaPattern) -> Self {
        let size = Size2us::from(data);
        assert!(
            size.width > 0 && size.height > 0,
            "a Bayer frame of {size} holds no samples"
        );
        let data = data.pixels();
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
