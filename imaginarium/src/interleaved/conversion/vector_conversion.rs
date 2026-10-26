//! [`VectorConversion`]: the vector kernel of a conversion, when its pair has one.

use simd::IsaTier;

use crate::color::color_format::ColorFormat;
use crate::interleaved::conversion::channels::ChannelPair;
use crate::interleaved::conversion::elements::ElementPair;

/// A conversion a vector kernel makes: a change of sample type, or one of channel count in `u8`.
/// Every other pair takes the scalar reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VectorConversion {
    Elements(ElementPair),
    Channels(ChannelPair),
}

impl VectorConversion {
    /// The kernel of a conversion from `from` to `to`, or `None` when the pair has none.
    pub(crate) fn of(from: ColorFormat, to: ColorFormat) -> Option<Self> {
        ElementPair::of(from, to)
            .map(Self::Elements)
            .or_else(|| ChannelPair::of(from, to).map(Self::Channels))
    }

    /// One packed row of `width` pixels, `src` into `dst`, on `tier`. `src` runs on to the end of
    /// the image.
    #[inline]
    pub(crate) fn convert_row(self, tier: IsaTier, src: &[u8], dst: &mut [u8], width: usize) {
        match self {
            Self::Elements(pair) => pair.convert_row(tier, src, dst),
            Self::Channels(pair) => pair.convert_row(tier, src, dst, width),
        }
    }
}
