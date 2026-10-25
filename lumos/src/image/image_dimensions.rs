//! Pixel extent plus channel count, validated once at construction.

use std::fmt;

use crate::image::channel_layout::ChannelLayout;
use crate::image::error::DimensionsError;
use common::Size2us;

/// Image dimensions: pixel size and the channels' layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ImageDimensions {
    size: Size2us,
    layout: ChannelLayout,
}

impl ImageDimensions {
    /// The longest side an image may have, 2³⁰ px — some 7000 times a large sensor's.
    ///
    /// Pixel addressing in the resampler and the detector runs in `i32` lanes, and a side this far
    /// under `i32::MAX` leaves every coordinate, and a kernel's reach past either edge, inside that
    /// range. The decoders refuse a larger image before it gets here.
    pub const MAX_SIDE: usize = 1 << 30;

    /// # Panics
    ///
    /// When `size` has a zero side or one past [`Self::MAX_SIDE`], or `size` and `layout` hold more
    /// samples than a `usize` counts.
    pub fn new(size: impl Into<Size2us>, layout: ChannelLayout) -> Self {
        Self::checked(size.into(), layout).unwrap_or_else(|error| panic!("{error}"))
    }

    /// The dimensions `size` and `layout` describe, or the first rule they break: a zero side, a
    /// side past [`Self::MAX_SIDE`], or more samples than a `usize` counts.
    pub const fn checked(size: Size2us, layout: ChannelLayout) -> Result<Self, DimensionsError> {
        let Size2us { width, height } = size;
        if width == 0 {
            return Err(DimensionsError::ZeroWidth);
        }
        if height == 0 {
            return Err(DimensionsError::ZeroHeight);
        }
        if width > Self::MAX_SIDE || height > Self::MAX_SIDE {
            return Err(DimensionsError::SidePastMax { size });
        }
        let channels = layout.count();
        if size.pixel_count().checked_mul(channels).is_none() {
            return Err(DimensionsError::SampleCount { size, channels });
        }
        Ok(Self { size, layout })
    }

    /// Pixel extent, without the channel count.
    pub const fn size(&self) -> Size2us {
        self.size
    }

    pub const fn width(&self) -> usize {
        self.size.width
    }

    pub const fn height(&self) -> usize {
        self.size.height
    }

    /// How many channel planes the image holds.
    pub const fn channels(&self) -> usize {
        self.layout.count()
    }

    pub const fn layout(&self) -> ChannelLayout {
        self.layout
    }

    /// Total number of f32 samples: `width * height * channels`.
    /// For a 100x100 RGB image, returns 30000.
    pub const fn sample_count(&self) -> usize {
        self.pixel_count()
            .checked_mul(self.channels())
            .expect("ImageDimensions validates sample count during construction")
    }

    /// Number of pixels: `width * height`.
    /// For a 100x100 RGB image, returns 10000.
    pub const fn pixel_count(&self) -> usize {
        self.size.pixel_count()
    }

    pub const fn is_grayscale(&self) -> bool {
        matches!(self.layout, ChannelLayout::Mono)
    }

    pub const fn is_rgb(&self) -> bool {
        matches!(self.layout, ChannelLayout::Rgb)
    }

    /// Bytes the image's pixels occupy, planar f32.
    pub(crate) const fn frame_bytes(&self) -> usize {
        self.sample_count() * size_of::<f32>()
    }

    /// Bytes a frame's flag plane adds: one per pixel, whatever its channel count. Charged to every
    /// frame, since a decoder that flags saturation, and a calibration that flags its repairs, give
    /// one to most frames.
    pub(crate) const fn flag_plane_bytes(&self) -> usize {
        self.pixel_count()
    }

    /// Bytes a frame holds as it decodes: its pixels and its flag plane.
    pub(crate) const fn decoded_bytes(&self) -> usize {
        self.frame_bytes() + self.flag_plane_bytes()
    }
}

/// `width×height×channels` — the form error messages quote geometry in, where the derived `Debug`
/// would spell out two nested structs.
impl fmt::Display for ImageDimensions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}x{}x{}", self.width(), self.height(), self.channels())
    }
}

#[cfg(test)]
mod tests {
    use crate::image::channel_layout::ChannelLayout;
    use crate::image::error::DimensionsError;
    use crate::image::image_dimensions::ImageDimensions;
    use common::Size2us;

    #[test]
    fn checked_names_the_first_rule_broken() {
        for layout in [ChannelLayout::Mono, ChannelLayout::Rgb] {
            let dimensions = ImageDimensions::new((4, 3), layout);
            let channels = layout.count();
            assert_eq!(dimensions.size(), (4, 3).into());
            assert_eq!(dimensions.channels(), channels);
            assert_eq!(dimensions.layout(), layout);
            assert_eq!(dimensions.pixel_count(), 12);
            assert_eq!(dimensions.sample_count(), 12 * channels);
            assert_eq!(dimensions.is_grayscale(), layout == ChannelLayout::Mono);
            assert_eq!(dimensions.is_rgb(), layout == ChannelLayout::Rgb);
        }

        let past = ImageDimensions::MAX_SIDE + 1;
        for (width, height, expected) in [
            (0, 3, DimensionsError::ZeroWidth),
            (0, 0, DimensionsError::ZeroWidth),
            (4, 0, DimensionsError::ZeroHeight),
            (
                past,
                1,
                DimensionsError::SidePastMax {
                    size: Size2us::new(past, 1),
                },
            ),
            (
                1,
                past,
                DimensionsError::SidePastMax {
                    size: Size2us::new(1, past),
                },
            ),
        ] {
            assert_eq!(
                ImageDimensions::checked(Size2us::new(width, height), ChannelLayout::Mono),
                Err(expected)
            );
        }
        for count in [0, 2, 4] {
            assert_eq!(
                ChannelLayout::try_from(count),
                Err(DimensionsError::Channels { count })
            );
        }
        assert_eq!(ChannelLayout::try_from(1), Ok(ChannelLayout::Mono));
        assert_eq!(ChannelLayout::try_from(3), Ok(ChannelLayout::Rgb));
    }
}
