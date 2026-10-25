//! [`PlanarDesc`]: a planar image's size and channel layout, valid by construction.

use std::fmt;

use common::Size2us;

use crate::color::color_format::{ChannelCount, ColorFormat, SampleType};
use crate::error::DescError;
use crate::interleaved::interleaved_desc::InterleavedDesc;
use crate::planar::channel_layout::ChannelLayout;

/// A planar image's size and channel layout: at least one pixel, and a byte count a `usize`
/// holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PlanarDesc {
    size: Size2us,
    layout: ChannelLayout,
}

impl PlanarDesc {
    /// # Panics
    ///
    /// When `size` has a zero side, or `size` and `layout` hold more bytes than a `usize`
    /// counts.
    pub fn new(size: impl Into<Size2us>, layout: ChannelLayout) -> Self {
        Self::checked(size.into(), layout).unwrap_or_else(|error| panic!("{error}"))
    }

    /// The descriptor `size` and `layout` make, or the first rule they break: a zero side, or more
    /// bytes than a `usize` counts.
    pub const fn checked(size: Size2us, layout: ChannelLayout) -> Result<Self, DescError> {
        let Size2us { width, height } = size;
        if width == 0 {
            return Err(DescError::ZeroWidth);
        }
        if height == 0 {
            return Err(DescError::ZeroHeight);
        }
        let pixel_byte_len = layout.count() * size_of::<f32>();
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
        Ok(Self { size, layout })
    }

    pub const fn size(&self) -> Size2us {
        self.size
    }

    pub const fn width(&self) -> usize {
        self.size.width
    }

    pub const fn height(&self) -> usize {
        self.size.height
    }

    pub const fn layout(&self) -> ChannelLayout {
        self.layout
    }

    /// How many planes the image holds.
    pub const fn channel_count(&self) -> usize {
        self.layout.count()
    }

    pub const fn pixel_count(&self) -> usize {
        self.size.pixel_count()
    }

    /// The samples of every plane: `width × height × channel_count`.
    pub const fn sample_count(&self) -> usize {
        self.pixel_count() * self.channel_count()
    }

    /// Bytes the planes occupy, an `f32` per sample.
    pub const fn byte_len(&self) -> usize {
        self.sample_count() * size_of::<f32>()
    }

    pub const fn is_mono(&self) -> bool {
        matches!(self.layout, ChannelLayout::Mono)
    }

    pub const fn is_rgb(&self) -> bool {
        matches!(self.layout, ChannelLayout::Rgb)
    }

    /// The interleaved format the planes repack to: `MONO_F32` or `RGB_F32`.
    pub const fn color_format(&self) -> ColorFormat {
        let channel_count = match self.layout {
            ChannelLayout::Mono => ChannelCount::Mono,
            ChannelLayout::Rgb => ChannelCount::Rgb,
        };
        ColorFormat::new(channel_count, SampleType::F32)
    }
}

/// `width×height×channel_count`, the form an error message quotes a geometry in.
impl fmt::Display for PlanarDesc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}x{}x{}",
            self.width(),
            self.height(),
            self.channel_count()
        )
    }
}

/// The same pixels interleaved, in [`PlanarDesc::color_format`].
impl From<PlanarDesc> for InterleavedDesc {
    fn from(desc: PlanarDesc) -> Self {
        Self::new(desc.size, desc.color_format())
    }
}

#[cfg(test)]
mod tests {
    use common::Size2us;

    use crate::color::color_format::ColorFormat;
    use crate::error::DescError;
    use crate::interleaved::interleaved_desc::InterleavedDesc;
    use crate::planar::channel_layout::ChannelLayout;
    use crate::planar::planar_desc::PlanarDesc;

    /// A 4 × 3 descriptor counts 12 pixels, and 12 or 36 samples of 4 bytes each.
    #[test]
    fn a_desc_counts_its_pixels_samples_and_bytes() {
        for (layout, samples, format) in [
            (ChannelLayout::Mono, 12, ColorFormat::MONO_F32),
            (ChannelLayout::Rgb, 36, ColorFormat::RGB_F32),
        ] {
            let desc = PlanarDesc::new((4, 3), layout);
            assert_eq!(desc.size(), Size2us::new(4, 3));
            assert_eq!(desc.layout(), layout);
            assert_eq!(desc.channel_count(), layout.count());
            assert_eq!(desc.pixel_count(), 12);
            assert_eq!(desc.sample_count(), samples);
            assert_eq!(desc.byte_len(), samples * 4);
            assert_eq!(desc.is_mono(), layout == ChannelLayout::Mono);
            assert_eq!(desc.is_rgb(), layout == ChannelLayout::Rgb);
            assert_eq!(desc.color_format(), format);
            assert_eq!(
                InterleavedDesc::from(desc),
                InterleavedDesc::new(Size2us::new(4, 3), format)
            );
        }
        assert_eq!(
            PlanarDesc::new((4, 3), ChannelLayout::Rgb).to_string(),
            "4x3x3"
        );
    }

    /// The first rule broken: a zero side, then a pixel count or a byte count past `usize`.
    /// `usize::MAX / 8 + 1` pixels in one row fit as mono, 4 bytes each, and as RGB, 12 bytes
    /// each, do not.
    #[test]
    fn checked_names_the_first_rule_broken() {
        let wide = Size2us::new(usize::MAX / 8 + 1, 1);
        let huge = Size2us::new(usize::MAX, 2);
        for (size, layout, expected) in [
            (
                Size2us::new(0, 3),
                ChannelLayout::Mono,
                DescError::ZeroWidth,
            ),
            (
                Size2us::new(0, 0),
                ChannelLayout::Mono,
                DescError::ZeroWidth,
            ),
            (
                Size2us::new(4, 0),
                ChannelLayout::Rgb,
                DescError::ZeroHeight,
            ),
            (
                huge,
                ChannelLayout::Mono,
                DescError::TooLarge {
                    size: huge,
                    pixel_byte_len: 4,
                },
            ),
            (
                wide,
                ChannelLayout::Rgb,
                DescError::TooLarge {
                    size: wide,
                    pixel_byte_len: 12,
                },
            ),
        ] {
            assert_eq!(PlanarDesc::checked(size, layout), Err(expected), "{size}");
        }
        assert!(PlanarDesc::checked(wide, ChannelLayout::Mono).is_ok());
    }

    #[test]
    #[should_panic(expected = "height must be positive")]
    fn new_panics_on_the_rule_broken() {
        PlanarDesc::new((4, 0), ChannelLayout::Mono);
    }
}
