//! The scalar reference every conversion kernel is checked against.

use crate::common::color_format::{ChannelCount, ColorFormat, SampleType};
use crate::common::luma;
use crate::common::sample::Sample;

/// A storage type's Rec. 709 luminance of one RGB triple, in that type.
pub(crate) trait Luminance: Sample {
    fn luminance(r: Self, g: Self, b: Self) -> Self;
}

impl Luminance for u8 {
    #[inline]
    fn luminance(r: Self, g: Self, b: Self) -> Self {
        let [wr, wg, wb] = luma::WEIGHTS_Q16;
        let sum = u32::from(r) * wr + u32::from(g) * wg + u32::from(b) * wb;
        luma::round_q16(u64::from(sum)) as Self
    }
}

impl Luminance for u16 {
    #[inline]
    fn luminance(r: Self, g: Self, b: Self) -> Self {
        let [wr, wg, wb] = luma::WEIGHTS_Q16.map(u64::from);
        let sum = u64::from(r) * wr + u64::from(g) * wg + u64::from(b) * wb;
        luma::round_q16(sum) as Self
    }
}

impl Luminance for f32 {
    #[inline]
    fn luminance(r: Self, g: Self, b: Self) -> Self {
        let [wr, wg, wb] = luma::WEIGHTS_F32;
        wr * r + wg * g + wb * b
    }
}

/// A row converter: one packed source row of `width` pixels into one packed destination row.
pub(crate) type ScalarRowFn = fn(src: &[u8], dst: &mut [u8], width: usize);

/// The scalar row converter for a pair of formats.
pub(crate) const fn row_converter(from: ColorFormat, to: ColorFormat) -> ScalarRowFn {
    match from.sample_type {
        SampleType::U8 => row_converter_from::<u8>(from.channel_count, to),
        SampleType::U16 => row_converter_from::<u16>(from.channel_count, to),
        SampleType::F32 => row_converter_from::<f32>(from.channel_count, to),
    }
}

const fn row_converter_from<S: Luminance>(from: ChannelCount, to: ColorFormat) -> ScalarRowFn {
    match to.sample_type {
        SampleType::U8 => row_converter_between::<S, u8>(from, to.channel_count),
        SampleType::U16 => row_converter_between::<S, u16>(from, to.channel_count),
        SampleType::F32 => row_converter_between::<S, f32>(from, to.channel_count),
    }
}

const fn row_converter_between<S: Luminance, D: Sample>(
    from: ChannelCount,
    to: ChannelCount,
) -> ScalarRowFn {
    match (from, to) {
        (ChannelCount::L, ChannelCount::L) => convert_row::<S, D, 1, 1>,
        (ChannelCount::L, ChannelCount::Rgb) => convert_row::<S, D, 1, 3>,
        (ChannelCount::L, ChannelCount::Rgba) => convert_row::<S, D, 1, 4>,
        (ChannelCount::Rgb, ChannelCount::L) => convert_row::<S, D, 3, 1>,
        (ChannelCount::Rgb, ChannelCount::Rgb) => convert_row::<S, D, 3, 3>,
        (ChannelCount::Rgb, ChannelCount::Rgba) => convert_row::<S, D, 3, 4>,
        (ChannelCount::Rgba, ChannelCount::L) => convert_row::<S, D, 4, 1>,
        (ChannelCount::Rgba, ChannelCount::Rgb) => convert_row::<S, D, 4, 3>,
        (ChannelCount::Rgba, ChannelCount::Rgba) => convert_row::<S, D, 4, 4>,
    }
}

/// One row, pixel by pixel. A source with fewer channels broadcasts grey and adds an opaque
/// alpha; a source with more drops alpha, and reduces colour to luminance in the source type
/// before the sample type changes.
pub(crate) fn convert_row<S: Luminance, D: Sample, const FROM: usize, const TO: usize>(
    src: &[u8],
    dst: &mut [u8],
    width: usize,
) {
    let src: &[S] = bytemuck::cast_slice(&src[..width * FROM * size_of::<S>()]);
    let dst: &mut [D] = bytemuck::cast_slice_mut(&mut dst[..width * TO * size_of::<D>()]);
    let (src, _) = src.as_chunks::<FROM>();
    let (dst, _) = dst.as_chunks_mut::<TO>();
    for (src, dst) in src.iter().zip(dst) {
        convert_pixel(src, dst);
    }
}

#[inline]
fn convert_pixel<S: Luminance, D: Sample, const FROM: usize, const TO: usize>(
    src: &[S; FROM],
    dst: &mut [D; TO],
) {
    match (FROM, TO) {
        (1, _) => {
            let grey = src[0].convert();
            dst[..TO.min(3)].fill(grey);
        }
        (_, 1) => dst[0] = S::luminance(src[0], src[1], src[2]).convert(),
        _ => {
            for (dst, &src) in dst.iter_mut().zip(&src[..3]) {
                *dst = src.convert();
            }
        }
    }
    if TO == 4 {
        dst[3] = if FROM == 4 {
            src[3].convert()
        } else {
            D::FULL_SCALE
        };
    }
}

/// Element by element through [`Sample::convert`] — the sub-vector tail of an element kernel,
/// so a tail can never disagree with the reference.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
pub(crate) fn convert_elements<S: Sample, D: Sample>(src: &[S], dst: &mut [D]) {
    debug_assert_eq!(src.len(), dst.len());
    for (dst, &src) in dst.iter_mut().zip(src) {
        *dst = src.convert();
    }
}
