//! [`Intensity`]: an image's combined intensity, read where it is needed.

use crate::image::flags::pixel_flags::PixelFlags;
use crate::image::flags::quality_flags::QualityFlags;
use crate::image::rgb::Rgb;
use crate::math::statistics::subsample::{MAX_STATISTIC_SAMPLES, Subsample};
use common::USize2;

/// An image's combined intensity, read a pixel at a time from its channels: the channel itself for
/// a mono image, [`Rgb::intensity`] of the three for RGB. Bit for bit the plane it replaces,
/// with no plane made.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Intensity<'a> {
    size: USize2,
    channels: Channels<'a>,
    /// The flags of the image the channels are, which name the pixels that hold a fill.
    flags: Option<&'a PixelFlags>,
}

/// The channels an [`Intensity`] is read from.
#[derive(Debug, Clone, Copy)]
enum Channels<'a> {
    Mono(&'a [f32]),
    Rgb([&'a [f32]; 3]),
}

impl<'a> Intensity<'a> {
    /// The intensity of a mono image of `size`: its one channel.
    pub(crate) fn mono(size: USize2, pixels: &'a [f32]) -> Self {
        debug_assert_eq!(pixels.len(), size.pixel_count());
        Self {
            size,
            channels: Channels::Mono(pixels),
            flags: None,
        }
    }

    /// The intensity of an RGB image of `size`: the mean of its three channels.
    pub(crate) fn rgb(size: USize2, [r, g, b]: [&'a [f32]; 3]) -> Self {
        debug_assert!(
            [r, g, b]
                .iter()
                .all(|plane| plane.len() == size.pixel_count())
        );
        Self {
            size,
            channels: Channels::Rgb([r, g, b]),
            flags: None,
        }
    }

    /// This intensity, of an image whose pixels carry `flags`.
    pub(crate) fn with_flags(self, flags: Option<&'a PixelFlags>) -> Self {
        debug_assert!(flags.is_none_or(|flags| flags.size() == self.size));
        Self { flags, ..self }
    }

    pub(crate) const fn size(&self) -> USize2 {
        self.size
    }

    /// The intensity of the pixel at row-major `idx`.
    #[inline]
    pub(crate) const fn at(&self, idx: usize) -> f32 {
        match self.channels {
            Channels::Mono(pixels) => pixels[idx],
            Channels::Rgb([r, g, b]) => Rgb {
                r: r[idx],
                g: g[idx],
                b: b[idx],
            }
            .intensity(),
        }
    }

    /// Whether the pixel at row-major `idx` holds a fill rather than data: it is flagged
    /// [`QualityFlags::NO_DATA`]. A statistic of the image leaves it out.
    #[inline]
    pub(crate) fn is_fill(&self, idx: usize) -> bool {
        self.flags
            .is_some_and(|flags| flags.at(idx).intersects(QualityFlags::NO_DATA))
    }

    /// How many pixels hold a fill.
    pub(crate) fn fill_count(&self) -> usize {
        self.flags.map_or(0, |flags| {
            usize::try_from(flags.count(QualityFlags::NO_DATA)).expect("a pixel count fits usize")
        })
    }

    /// The values of the pixels holding data an image statistic is taken from: at most
    /// [`MAX_STATISTIC_SAMPLES`], see [`Subsample::data_values`].
    pub(crate) fn statistic_values(&self) -> Vec<f32> {
        Subsample::data_values(
            self.size.pixel_count(),
            self.fill_count(),
            MAX_STATISTIC_SAMPLES,
            |idx| self.is_fill(idx),
            |idx| self.at(idx),
        )
    }
}
