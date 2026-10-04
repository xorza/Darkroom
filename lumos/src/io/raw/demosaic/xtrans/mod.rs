//! X-Trans CFA demosaicing module.
//!
//! Provides demosaicing for Fujifilm X-Trans sensors which use a 6x6 CFA pattern
//! instead of the standard 2x2 Bayer pattern.
//!
//! The X-Trans pattern has ~55% green, ~22.5% red, and ~22.5% blue pixels arranged
//! so that every row and column contains all three colors.
//!
//! Uses the Markesteijn algorithm, one pass or three: directional interpolation with
//! homogeneity-based selection.

mod hex_lookup;
pub(crate) mod markesteijn;
pub(crate) mod xtrans_pattern;

use std::time::Instant;

use common::CancelToken;

use crate::io::cancelled::Cancelled;
use crate::io::raw::demosaic::xtrans::markesteijn::MarkesteijnPasses;
use crate::io::raw::demosaic::xtrans::xtrans_pattern::XTransPattern;
use crate::math::size2us::Size2us;

/// Demosaic a calibrated X-Trans frame — samples may lie outside `[0, 1]` — into planar
/// `[R, G, B]` channels of its size.
pub(crate) fn demosaic(
    data: &[f32],
    size: Size2us,
    pattern: XTransPattern,
    passes: MarkesteijnPasses,
    cancel: &CancelToken,
) -> Result<[Vec<f32>; 3], Cancelled> {
    let xtrans = XTransImage::new(data, size, pattern);
    let demosaic_start = Instant::now();
    let rgb_pixels = markesteijn::demosaic(&xtrans, passes, cancel)?;
    tracing::info!(
        "X-Trans Markesteijn demosaicing {}x{} took {:.2}ms",
        size.width,
        size.height,
        demosaic_start.elapsed().as_secs_f64() * 1000.0
    );
    Ok(rgb_pixels)
}

/// An X-Trans frame the demosaic reads: its samples row by row, its size, and its pattern
/// anchored at its first pixel.
#[derive(Debug)]
pub(crate) struct XTransImage<'a> {
    data: &'a [f32],
    pub(crate) size: Size2us,
    pub(crate) pattern: XTransPattern,
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
        Self {
            data,
            size,
            pattern,
        }
    }

    /// The sample at `(x, y)`.
    #[inline(always)]
    pub(crate) const fn read(&self, y: usize, x: usize) -> f32 {
        self.data[y * self.size.width + x]
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::internals::cfa::XTRANS_PATTERN;
    use crate::io::raw::demosaic::xtrans::XTransImage;
    use crate::io::raw::demosaic::xtrans::xtrans_pattern::XTransPattern;
    use crate::math::size2us::Size2us;

    /// `data` as a frame of `size` under the test pattern.
    pub(crate) fn make_xtrans(data: &[f32], size: Size2us) -> XTransImage<'_> {
        XTransImage::new(data, size, XTRANS_PATTERN)
    }

    pub(crate) fn test_pattern_array() -> [[u8; 6]; 6] {
        *XTRANS_PATTERN.rows()
    }

    pub(crate) fn test_pattern() -> XTransPattern {
        XTRANS_PATTERN
    }
}

#[cfg(test)]
mod tests;
