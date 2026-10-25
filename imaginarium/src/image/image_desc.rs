use std::fmt;

use crate::common::color_format::ColorFormat;
use crate::common::error::{Error, Result};

/// Image dimensions + pixel format. Pixel data is **always tightly packed**
/// (`row_bytes == width * bytes_per_pixel`, no inter-row padding) — any row
/// alignment a GPU backend needs lives inside `GpuImage` (`src/gpu/`), never here.
#[derive(Clone, Copy, Eq, PartialEq, Debug, Hash)]
pub struct ImageDesc {
    pub width: usize,
    pub height: usize,
    pub color_format: ColorFormat,
}

impl ImageDesc {
    /// Create a new (tightly packed) image descriptor.
    pub const fn new(width: usize, height: usize, color_format: ColorFormat) -> Self {
        Self {
            width,
            height,
            color_format,
        }
    }

    /// Total packed byte size: `height * row_bytes`.
    pub const fn size_in_bytes(&self) -> usize {
        self.height * self.row_bytes()
    }

    /// Bytes per (packed) row: `width * bytes_per_pixel`.
    pub const fn row_bytes(&self) -> usize {
        self.width * self.color_format.byte_count()
    }

    /// Panics unless `other` covers the same pixel grid in the same format —
    /// the precondition every two-image operation shares. Because descriptors
    /// are exactly grid plus format, equality is that whole predicate.
    ///
    /// `pair` names the two images in the panic message, e.g. `"src/output"`.
    #[track_caller]
    pub(crate) fn assert_same(self, other: Self, pair: &str) {
        assert_eq!(self, other, "{pair} descriptor mismatch");
    }

    /// Checks that the descriptor names at least one pixel and that its byte size fits in
    /// `usize`, so [`Self::size_in_bytes`] cannot overflow.
    pub(crate) fn validate(&self) -> Result<()> {
        if self.width == 0 || self.height == 0 {
            return Err(Error::SizeMismatch(format!(
                "image dimensions must be non-zero, got {}x{}",
                self.width, self.height
            )));
        }
        let fits = self
            .width
            .checked_mul(self.height)
            .and_then(|pixels| pixels.checked_mul(self.color_format.byte_count()));
        if fits.is_none() {
            return Err(Error::SizeMismatch(format!(
                "{}x{} {} does not fit in memory",
                self.width, self.height, self.color_format
            )));
        }
        Ok(())
    }
}

impl fmt::Display for ImageDesc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}x{} {}", self.width, self.height, self.color_format)
    }
}
