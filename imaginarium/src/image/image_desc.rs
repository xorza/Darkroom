use std::fmt;

use common::Size2us;

use crate::color::color_format::ColorFormat;
use crate::error::DescError;

/// An interleaved image's size and pixel format: at least one pixel, and a byte count a `usize`
/// holds. Pixel data is **always tightly packed** (`row_byte_len == width * bytes_per_pixel`, no
/// inter-row padding) — any row alignment a GPU backend needs lives inside `GpuImage`
/// (`src/gpu/`), never here.
#[derive(Clone, Copy, Eq, PartialEq, Debug, Hash)]
pub struct InterleavedDesc {
    size: Size2us,
    color_format: ColorFormat,
}

impl InterleavedDesc {
    /// # Panics
    ///
    /// When `size` has a zero side, or its pixels have more bytes than a `usize` counts.
    pub const fn new(size: Size2us, color_format: ColorFormat) -> Self {
        match Self::checked(size, color_format) {
            Ok(desc) => desc,
            Err(DescError::ZeroWidth) => panic!("width must be positive"),
            Err(DescError::ZeroHeight) => panic!("height must be positive"),
            Err(_) => panic!("the pixels' bytes overflow usize"),
        }
    }

    /// The descriptor `size` and `color_format` make, or the first rule they break: a zero side,
    /// or more bytes than a `usize` counts.
    pub const fn checked(size: Size2us, color_format: ColorFormat) -> Result<Self, DescError> {
        let Size2us { width, height } = size;
        if width == 0 {
            return Err(DescError::ZeroWidth);
        }
        if height == 0 {
            return Err(DescError::ZeroHeight);
        }
        let pixel_byte_len = color_format.byte_len();
        let byte_len = match width.checked_mul(height) {
            Some(pixel_count) => pixel_count.checked_mul(pixel_byte_len),
            None => None,
        };
        if byte_len.is_none() {
            return Err(DescError::TooLarge {
                size,
                pixel_byte_len,
            });
        }
        Ok(Self { size, color_format })
    }

    pub const fn size(&self) -> Size2us {
        self.size
    }

    pub const fn color_format(&self) -> ColorFormat {
        self.color_format
    }

    /// Total packed byte length: `height * row_byte_len`.
    pub const fn byte_len(&self) -> usize {
        self.size.height * self.row_byte_len()
    }

    /// Bytes per (packed) row: `width * bytes_per_pixel`.
    pub const fn row_byte_len(&self) -> usize {
        self.size.width * self.color_format.byte_len()
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
}

impl fmt::Display for InterleavedDesc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.size, self.color_format)
    }
}

#[cfg(test)]
mod tests {
    use common::Size2us;

    use crate::color::color_format::ColorFormat;
    use crate::error::DescError;
    use crate::image::image_desc::InterleavedDesc;

    /// A 5 × 3 `RGB_U16` descriptor: rows of 5 · 6 = 30 bytes, 90 in all.
    #[test]
    fn a_desc_counts_its_bytes() {
        let desc = InterleavedDesc::new(Size2us::new(5, 3), ColorFormat::RGB_U16);
        assert_eq!(desc.size(), Size2us::new(5, 3));
        assert_eq!(desc.color_format(), ColorFormat::RGB_U16);
        assert_eq!(desc.row_byte_len(), 30);
        assert_eq!(desc.byte_len(), 90);
        assert_eq!(desc.to_string(), "5x3 RGB u16");
    }

    /// The first rule broken: a zero side, then a pixel count or a byte count past `usize`.
    /// `usize::MAX / 4 + 1` pixels in one row fit as `MONO_U8`, and as `RGBA_U8` do not.
    #[test]
    fn checked_names_the_first_rule_broken() {
        let wide = Size2us::new(usize::MAX / 4 + 1, 1);
        let huge = Size2us::new(usize::MAX, 2);
        for (size, format, expected) in [
            (
                Size2us::new(0, 3),
                ColorFormat::MONO_U8,
                DescError::ZeroWidth,
            ),
            (
                Size2us::new(0, 0),
                ColorFormat::MONO_U8,
                DescError::ZeroWidth,
            ),
            (
                Size2us::new(4, 0),
                ColorFormat::RGB_F32,
                DescError::ZeroHeight,
            ),
            (
                huge,
                ColorFormat::MONO_U8,
                DescError::TooLarge {
                    size: huge,
                    pixel_byte_len: 1,
                },
            ),
            (
                wide,
                ColorFormat::RGBA_U8,
                DescError::TooLarge {
                    size: wide,
                    pixel_byte_len: 4,
                },
            ),
        ] {
            assert_eq!(
                InterleavedDesc::checked(size, format),
                Err(expected),
                "{size}"
            );
        }
        assert!(InterleavedDesc::checked(wide, ColorFormat::MONO_U8).is_ok());
    }

    #[test]
    #[should_panic(expected = "height must be positive")]
    fn new_panics_on_the_rule_broken() {
        InterleavedDesc::new(Size2us::new(4, 0), ColorFormat::MONO_U8);
    }
}
