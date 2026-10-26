//! [`ChannelPair`]: a change of channel count in `u8`, as one vector kernel per pair: RGBA ↔ RGB,
//! RGB(A) → mono and mono → RGB(A).
//!
//! Every kernel is bit-identical to the scalar reference, a row's last partial group included,
//! which runs through the same step. The luma kernels keep the reference's own Rec. 709 weights and
//! its 32-bit sum, not the 8-bit approximation a 16-bit sum would force, and round the sum as
//! [`luma::round_q16`] does.

use simd::{Isa, IsaTier, Kernel, U8_LANES, U8x32};

use crate::color::color_format::{ChannelCount, ColorFormat, SampleType};
use crate::color::luma;
use crate::interleaved::row_groups::GroupStep;

/// A change of channel count an `u8` row kernel makes, picked once per image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChannelPair {
    RgbaToRgb,
    RgbToRgba,
    RgbaToMono,
    RgbToMono,
    MonoToRgba,
    MonoToRgb,
}

impl ChannelPair {
    /// The pair a conversion from `from` to `to` is, or `None` when it is not between two `u8`
    /// formats of different channel counts.
    pub(crate) const fn of(from: ColorFormat, to: ColorFormat) -> Option<Self> {
        if !matches!(from.sample_type, SampleType::U8) || !matches!(to.sample_type, SampleType::U8)
        {
            return None;
        }
        Some(match (from.channel_count, to.channel_count) {
            (ChannelCount::Rgba, ChannelCount::Rgb) => Self::RgbaToRgb,
            (ChannelCount::Rgb, ChannelCount::Rgba) => Self::RgbToRgba,
            (ChannelCount::Rgba, ChannelCount::Mono) => Self::RgbaToMono,
            (ChannelCount::Rgb, ChannelCount::Mono) => Self::RgbToMono,
            (ChannelCount::Mono, ChannelCount::Rgba) => Self::MonoToRgba,
            (ChannelCount::Mono, ChannelCount::Rgb) => Self::MonoToRgb,
            (ChannelCount::Mono, ChannelCount::Mono)
            | (ChannelCount::Rgb, ChannelCount::Rgb)
            | (ChannelCount::Rgba, ChannelCount::Rgba) => return None,
        })
    }

    /// One packed row of `width` pixels, `src` into `dst`, on `tier`. `src` runs on to the end of
    /// the image.
    #[inline]
    pub(crate) fn convert_row(self, tier: IsaTier, src: &[u8], dst: &mut [u8], width: usize) {
        match self {
            Self::RgbaToRgb => tier.run(Channels::<4, 3>::new(src, dst, width)),
            Self::RgbToRgba => tier.run(Channels::<3, 4>::new(src, dst, width)),
            Self::RgbaToMono => tier.run(Channels::<4, 1>::new(src, dst, width)),
            Self::RgbToMono => tier.run(Channels::<3, 1>::new(src, dst, width)),
            Self::MonoToRgba => tier.run(Channels::<1, 4>::new(src, dst, width)),
            Self::MonoToRgb => tier.run(Channels::<1, 3>::new(src, dst, width)),
        }
    }
}

/// One packed `u8` row of `FROM`-channel pixels into `TO`-channel pixels.
#[derive(Debug)]
struct Channels<'a, const FROM: usize, const TO: usize> {
    src: &'a [u8],
    dst: &'a mut [u8],
}

impl<'a, const FROM: usize, const TO: usize> Channels<'a, FROM, TO> {
    fn new(src: &'a [u8], dst: &'a mut [u8], width: usize) -> Self {
        Self {
            src: &src[..width * FROM],
            dst: &mut dst[..width * TO],
        }
    }
}

/// A kernel of `FROM` to `TO` channels: its row through its [`GroupStep`] of thirty-two pixels.
macro_rules! channel_kernel {
    ($from:literal, $to:literal) => {
        impl Kernel for Channels<'_, $from, $to> {
            type Output = ();

            #[inline(always)]
            fn run<V: Isa>(self, isa: V) {
                <Self as GroupStep<u8, u8, { $from * U8_LANES }, { $to * U8_LANES }>>::run_row(
                    isa, self.src, self.dst,
                );
            }
        }
    };
}

channel_kernel!(4, 3);
channel_kernel!(3, 4);
channel_kernel!(4, 1);
channel_kernel!(3, 1);
channel_kernel!(1, 4);
channel_kernel!(1, 3);

impl GroupStep<u8, u8, { 4 * U8_LANES }, { 3 * U8_LANES }> for Channels<'_, 4, 3> {
    #[inline(always)]
    fn step<V: Isa>(isa: V, group: &[u8; 4 * U8_LANES], out: &mut [u8; 3 * U8_LANES]) {
        isa.drop_fourth_u8(group, out);
    }
}

impl GroupStep<u8, u8, { 3 * U8_LANES }, { 4 * U8_LANES }> for Channels<'_, 3, 4> {
    #[inline(always)]
    fn step<V: Isa>(isa: V, group: &[u8; 3 * U8_LANES], out: &mut [u8; 4 * U8_LANES]) {
        isa.append_fourth_u8(group, u8::MAX, out);
    }
}

impl GroupStep<u8, u8, { 4 * U8_LANES }, U8_LANES> for Channels<'_, 4, 1> {
    #[inline(always)]
    fn step<V: Isa>(isa: V, group: &[u8; 4 * U8_LANES], out: &mut [u8; U8_LANES]) {
        isa.weighted_sum3_of4_u8(group, WEIGHTS).store(out);
    }
}

impl GroupStep<u8, u8, { 3 * U8_LANES }, U8_LANES> for Channels<'_, 3, 1> {
    #[inline(always)]
    fn step<V: Isa>(isa: V, group: &[u8; 3 * U8_LANES], out: &mut [u8; U8_LANES]) {
        isa.weighted_sum3_u8(group, WEIGHTS).store(out);
    }
}

impl GroupStep<u8, u8, U8_LANES, { 4 * U8_LANES }> for Channels<'_, 1, 4> {
    #[inline(always)]
    fn step<V: Isa>(isa: V, group: &[u8; U8_LANES], out: &mut [u8; 4 * U8_LANES]) {
        let mono = isa.load_u8(group);
        isa.store_interleaved_u8([mono, mono, mono, isa.splat_u8(u8::MAX)], out);
    }
}

impl GroupStep<u8, u8, U8_LANES, { 3 * U8_LANES }> for Channels<'_, 1, 3> {
    #[inline(always)]
    fn step<V: Isa>(isa: V, group: &[u8; U8_LANES], out: &mut [u8; 3 * U8_LANES]) {
        let mono = isa.load_u8(group);
        isa.store_interleaved_u8([mono, mono, mono], out);
    }
}

/// [`luma::WEIGHTS_Q16`] as the weighted sums take them, which round out of Q16 as
/// [`luma::round_q16`] does.
const WEIGHTS: [u16; 3] = {
    let [r, g, b] = luma::WEIGHTS_Q16;
    assert!(r + g + b == 1 << 16, "the weights sum to one in Q16");
    assert!(
        r <= 32767 && b <= 32767 && g <= 65534,
        "within the weighted sum's bounds"
    );
    [r as u16, g as u16, b as u16]
};
