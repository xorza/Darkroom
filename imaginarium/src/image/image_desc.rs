use std::fmt;

use common::Size2us;

use crate::color::color_format::ColorFormat;
use crate::error::{Error, Result};

/// Image dimensions + pixel format. Pixel data is **always tightly packed**
/// (`row_bytes == width * bytes_per_pixel`, no inter-row padding) — any row
/// alignment a GPU backend needs lives inside `GpuImage` (`src/gpu/`), never here.
#[derive(Clone, Copy, Eq, PartialEq, Debug, Hash)]
pub struct ImageDesc {
    pub size: Size2us,
    pub color_format: ColorFormat,
}

impl ImageDesc {
    /// Create a new (tightly packed) image descriptor.
    pub const fn new(size: Size2us, color_format: ColorFormat) -> Self {
        Self { size, color_format }
    }

    /// Total packed byte size: `height * row_bytes`.
    pub const fn size_in_bytes(&self) -> usize {
        self.size.height * self.row_bytes()
    }

    /// Bytes per (packed) row: `width * bytes_per_pixel`.
    pub const fn row_bytes(&self) -> usize {
        self.size.width * self.color_format.byte_count()
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
        let Size2us { width, height } = self.size;
        if width == 0 || height == 0 {
            return Err(Error::SizeMismatch(format!(
                "image dimensions must be non-zero, got {}",
                self.size
            )));
        }
        let fits = width
            .checked_mul(height)
            .and_then(|pixels| pixels.checked_mul(self.color_format.byte_count()));
        if fits.is_none() {
            return Err(Error::SizeMismatch(format!(
                "{} {} does not fit in memory",
                self.size, self.color_format
            )));
        }
        Ok(())
    }
}

impl fmt::Display for ImageDesc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.size, self.color_format)
    }
}
