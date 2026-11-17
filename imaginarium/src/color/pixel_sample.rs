//! [`PixelSample`]: one pixel's channels, in the type its image stores them in.

use std::fmt;
use std::fmt::{Display, Formatter};

/// One pixel's channels in its image's own sample type: [`ColorFormat`]'s nine products as
/// values, so none is invalid.
///
/// [`ColorFormat`]: crate::ColorFormat
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PixelSample {
    U8(Channels<u8>),
    U16(Channels<u16>),
    F32(Channels<f32>),
}

/// The channels of one pixel, as its [`ChannelCount`](crate::ChannelCount) holds them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channels<T> {
    Mono(T),
    Rgb([T; 3]),
    Rgba([T; 4]),
}

impl<T: Copy> Channels<T> {
    /// The channels of a pixel stored as `values`: one, three or four of them.
    pub(crate) fn of(values: &[T]) -> Self {
        match *values {
            [mono] => Self::Mono(mono),
            [r, g, b] => Self::Rgb([r, g, b]),
            [r, g, b, a] => Self::Rgba([r, g, b, a]),
            _ => panic!("a pixel holds 1, 3 or 4 channels, not {}", values.len()),
        }
    }

    /// The channels in their stored order.
    pub const fn as_slice(&self) -> &[T] {
        match self {
            Self::Mono(mono) => std::slice::from_ref(mono),
            Self::Rgb(rgb) => rgb,
            Self::Rgba(rgba) => rgba,
        }
    }
}

/// The places an `f32` sample is written to: a 16-bit frame's step, `1 / 65535`, is `0.0000153`,
/// so five places tell apart what a `u16` source told apart to within one step.
const F32_PLACES: usize = 5;

/// Each channel in its stored order, a space between: an integer type as its stored integer and
/// an `f32` to five places.
impl Display for PixelSample {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        fn join<T>(
            f: &mut Formatter<'_>,
            values: &[T],
            write: impl Fn(&mut Formatter<'_>, &T) -> fmt::Result,
        ) -> fmt::Result {
            for (idx, value) in values.iter().enumerate() {
                if idx > 0 {
                    f.write_str(" ")?;
                }
                write(f, value)?;
            }
            Ok(())
        }
        match self {
            Self::U8(channels) => join(f, channels.as_slice(), |f, v| write!(f, "{v}")),
            Self::U16(channels) => join(f, channels.as_slice(), |f, v| write!(f, "{v}")),
            Self::F32(channels) => {
                join(f, channels.as_slice(), |f, v| write!(f, "{v:.F32_PLACES$}"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::color::pixel_sample::{Channels, PixelSample};

    /// An integer reads as stored, an `f32` to five places, each channel in order with a space
    /// between; a NaN reads as one.
    #[test]
    fn a_sample_writes_each_channel_in_its_own_type() {
        let cases = [
            (PixelSample::U8(Channels::Mono(255)), "255"),
            (
                PixelSample::U16(Channels::Rgb([0, 257, 65535])),
                "0 257 65535",
            ),
            // 0.031_27 is 0.031269999… in f32, which rounds back to five places.
            (PixelSample::F32(Channels::Mono(0.031_27)), "0.03127"),
            (
                PixelSample::F32(Channels::Rgba([1.0, -0.5, 1.0e-6, f32::NAN])),
                "1.00000 -0.50000 0.00000 NaN",
            ),
        ];
        for (sample, text) in cases {
            assert_eq!(sample.to_string(), text);
        }
    }

    #[test]
    fn channels_come_from_one_three_or_four_values() {
        assert_eq!(Channels::of(&[7_u8]), Channels::Mono(7));
        assert_eq!(Channels::of(&[1_u8, 2, 3]), Channels::Rgb([1, 2, 3]));
        assert_eq!(Channels::of(&[1_u8, 2, 3, 4]).as_slice(), [1, 2, 3, 4]);
    }
}
