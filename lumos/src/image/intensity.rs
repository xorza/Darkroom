//! [`Intensity`]: an image's combined intensity, read where it is needed.

use crate::image::rgb::Rgb;
use common::Size2us;

/// An image's combined intensity, read a pixel at a time from its channels: the channel itself for
/// a grayscale image, [`Rgb::intensity`] of the three for RGB. Bit for bit the plane it replaces,
/// with no plane made.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Intensity<'a> {
    size: Size2us,
    channels: Channels<'a>,
}

/// The channels an [`Intensity`] is read from.
#[derive(Debug, Clone, Copy)]
enum Channels<'a> {
    Mono(&'a [f32]),
    Rgb([&'a [f32]; 3]),
}

impl<'a> Intensity<'a> {
    /// The intensity of a grayscale image of `size`: its one channel.
    pub(crate) fn mono(size: Size2us, pixels: &'a [f32]) -> Self {
        debug_assert_eq!(pixels.len(), size.pixel_count());
        Self {
            size,
            channels: Channels::Mono(pixels),
        }
    }

    /// The intensity of an RGB image of `size`: the mean of its three channels.
    pub(crate) fn rgb(size: Size2us, [r, g, b]: [&'a [f32]; 3]) -> Self {
        debug_assert!(
            [r, g, b]
                .iter()
                .all(|plane| plane.len() == size.pixel_count())
        );
        Self {
            size,
            channels: Channels::Rgb([r, g, b]),
        }
    }

    pub(crate) const fn size(&self) -> Size2us {
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
}
