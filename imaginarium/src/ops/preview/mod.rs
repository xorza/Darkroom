mod cpu;

use common::Size2us;

use crate::interleaved::interleaved_image::InterleavedImage;
use crate::planar::planar_image::PlanarImage;

/// Generates a downscaled `RGBA_U8` **preview** of an image in one fused pass —
/// area-average downscale and format conversion at the same time, for fast
/// thumbnail generation.
///
/// Each output pixel box-averages the source pixels in its footprint (area
/// sampling — the quality-correct filter for shrinking, where bilinear's 4 taps
/// would alias), so every source pixel is read exactly once. The output is
/// always `RGBA_U8`; mono and RGB sources get an opaque alpha. A source with alpha weighs each
/// pixel's color by its alpha, so a transparent pixel lends a footprint no color.
#[derive(Debug, Clone, Copy)]
pub struct Preview {
    pub size: Size2us,
}

impl Preview {
    pub const fn new(size: Size2us) -> Self {
        Self { size }
    }

    /// Area-averages `input` down to the target size and converts it to `RGBA_U8`.
    ///
    /// The target is clamped to at least 1×1. If the target is *larger* than the
    /// source in a dimension, that axis degrades to nearest sampling (a preview
    /// never upscales, so this is just a safety net).
    pub fn to_rgba8(&self, input: &InterleavedImage) -> InterleavedImage {
        cpu::generate(self, input)
    }

    /// [`Self::to_rgba8`] of `f32` planes, read where they lie rather than interleaved first.
    pub fn planes_to_rgba8(&self, input: &PlanarImage) -> InterleavedImage {
        cpu::generate_planar(self, input)
    }
}

#[cfg(feature = "internals")]
pub(crate) mod bench;
