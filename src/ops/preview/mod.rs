#[cfg(feature = "bench")]
pub(crate) mod bench;
mod cpu;

use crate::image::Image;

/// Generates a downscaled `RGBA_U8` **preview** of an image in one fused pass —
/// area-average downscale and format conversion at the same time, for fast
/// thumbnail generation.
///
/// Each output pixel box-averages the source pixels in its footprint (area
/// sampling — the quality-correct filter for shrinking, where bilinear's 4 taps
/// would alias), so every source pixel is read exactly once. The output is
/// always `RGBA_U8`; grayscale and RGB sources get an opaque alpha.
#[derive(Debug, Clone, Copy)]
pub struct Preview {
    pub width: usize,
    pub height: usize,
}

impl Preview {
    pub fn new(width: usize, height: usize) -> Self {
        Self { width, height }
    }

    /// Area-averages `input` down to the target size and converts it to `RGBA_U8`.
    ///
    /// The target is clamped to at least 1×1. If the target is *larger* than the
    /// source in a dimension, that axis degrades to nearest sampling (a preview
    /// never upscales, so this is just a safety net).
    pub fn to_rgba8(&self, input: &Image) -> Image {
        cpu::generate(self, input)
    }
}
