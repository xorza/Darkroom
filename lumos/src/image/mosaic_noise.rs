//! [`MosaicNoise`]: the white noise of each color of the mosaic a frame was demosaiced from.

use serde::{Deserialize, Serialize};

use crate::image::background_split::BackgroundSplit;

/// The white noise of each color of the mosaic a frame was demosaiced from, measured before the
/// interpolation correlated neighbouring pixels: a measurement of the demosaiced frame reads less
/// noise than its sensor has. PixInsight's Debayer measures its noise estimates at the same point.
///
/// Slot `c` is color `c` of the pattern, which is channel `c` of the demosaiced frame. The values
/// are in the samples' own units and in the sensor's balance, which the demosaic keeps.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MosaicNoise {
    /// Each color's white-noise variance where no flat amplified it, over the photosites no flag
    /// names, split by how a flat amplified it: unflattened for a mosaic no flat divided.
    pub background: [BackgroundSplit; 3],
    /// Each color's median, the level `background` was measured at.
    pub sky: [f32; 3],
    /// The mosaic's quantization σ, the floor of `sigma`. The demosaic clears the frame's own.
    pub quantization_sigma: Option<f32>,
}
