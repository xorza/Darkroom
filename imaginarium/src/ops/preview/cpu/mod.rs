use std::cmp::Ordering;
use std::ops::AddAssign;

use common::{Size2us, par};
use rayon::prelude::*;

use crate::color::color_format::{ChannelCount, ColorFormat, SampleType};
use crate::color::sample::Sample;
use crate::image::InterleavedImage;
use crate::image::image_desc::InterleavedDesc;
use crate::ops::preview::Preview;

/// Area-averages `input` to `params`' target size, fused with the conversion to
/// `RGBA_U8`. See [`Preview`] for the algorithm.
pub(super) fn generate(params: &Preview, input: &InterleavedImage) -> InterleavedImage {
    let dst_w = params.size.width.max(1);
    let dst_h = params.size.height.max(1);
    let format = input.desc().color_format;
    match (format.sample_type, format.channel_count) {
        (SampleType::U8, ChannelCount::Mono) => typed::<u8, 1>(input, dst_w, dst_h),
        (SampleType::U8, ChannelCount::Rgb) => typed::<u8, 3>(input, dst_w, dst_h),
        (SampleType::U8, ChannelCount::Rgba) => typed::<u8, 4>(input, dst_w, dst_h),
        (SampleType::U16, ChannelCount::Mono) => typed::<u16, 1>(input, dst_w, dst_h),
        (SampleType::U16, ChannelCount::Rgb) => typed::<u16, 3>(input, dst_w, dst_h),
        (SampleType::U16, ChannelCount::Rgba) => typed::<u16, 4>(input, dst_w, dst_h),
        (SampleType::F32, ChannelCount::Mono) => typed::<f32, 1>(input, dst_w, dst_h),
        (SampleType::F32, ChannelCount::Rgb) => typed::<f32, 3>(input, dst_w, dst_h),
        (SampleType::F32, ChannelCount::Rgba) => typed::<f32, 4>(input, dst_w, dst_h),
    }
}

/// A source sample type: the sum a footprint accumulates in, and how a footprint's sum turns
/// into an output byte.
///
/// Integer samples sum exactly in `u64` and narrow by an exact rational rounding; `f32` sums in
/// `f64`. Either way the byte is the footprint mean narrowed as [`Sample::convert`] narrows a
/// single value — so a one-pixel footprint is exactly the format conversion.
trait PreviewSample: Sample {
    type Sum: Copy + Default + AddAssign;
    fn widen(self) -> Self::Sum;
    fn to_u8(sum: Self::Sum, count: u64) -> u8;
}

impl PreviewSample for u8 {
    type Sum = u64;

    #[inline]
    fn widen(self) -> u64 {
        u64::from(self)
    }

    fn to_u8(sum: u64, count: u64) -> u8 {
        narrow(round_div(sum, count))
    }
}

impl PreviewSample for u16 {
    type Sum = u64;

    #[inline]
    fn widen(self) -> u64 {
        u64::from(self)
    }

    /// `mean · 255 / 65535` is `sum / (257 · count)`.
    fn to_u8(sum: u64, count: u64) -> u8 {
        narrow(round_div(sum, 257 * count))
    }
}

impl PreviewSample for f32 {
    type Sum = f64;

    #[inline]
    fn widen(self) -> f64 {
        f64::from(self)
    }

    #[expect(
        clippy::cast_sign_loss,
        reason = "the saturating `as` cast is the narrowing rule: negatives and NaN to zero, overflow to the maximum"
    )]
    fn to_u8(sum: f64, count: u64) -> u8 {
        (sum / count as f64 * 255.0).round_ties_even() as u8
    }
}

/// `numerator / denominator` rounded to the nearest integer, ties to even.
fn round_div(numerator: u64, denominator: u64) -> u64 {
    let (quotient, remainder) = (numerator / denominator, numerator % denominator);
    match (2 * remainder).cmp(&denominator) {
        Ordering::Less => quotient,
        Ordering::Greater => quotient + 1,
        Ordering::Equal => quotient + (quotient & 1),
    }
}

/// A rounded mean of values that are themselves at most 255.
fn narrow(value: u64) -> u8 {
    u8::try_from(value).expect("a mean never exceeds its largest value")
}

fn typed<T: PreviewSample, const N: usize>(
    input: &InterleavedImage,
    dst_w: usize,
    dst_h: usize,
) -> InterleavedImage {
    let Size2us {
        width: src_w,
        height: src_h,
    } = input.desc().size;
    let src: &[T] = bytemuck::cast_slice(input.bytes());

    let mut out = InterleavedImage::new_black(InterleavedDesc::new(
        Size2us::new(dst_w, dst_h),
        ColorFormat::RGBA_U8,
    ))
    .expect("RGBA_U8 preview dims are valid");

    par::chunks_mut(out.bytes_mut(), dst_w * 4)
        .enumerate()
        .for_each(|(oy, out_row)| {
            // Source-row band for this output row; the `.max(+1)` guards the
            // upscale corner case (empty footprint), `.min` keeps it in bounds.
            let sy0 = oy * src_h / dst_h;
            let sy1 = (((oy + 1) * src_h / dst_h).max(sy0 + 1)).min(src_h);

            for (ox, out) in out_row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let sx0 = ox * src_w / dst_w;
                let sx1 = (((ox + 1) * src_w / dst_w).max(sx0 + 1)).min(src_w);

                let mut sum = [T::Sum::default(); N];
                for sy in sy0..sy1 {
                    let row = sy * src_w;
                    // The footprint's columns are contiguous in the row; slice the
                    // band once (one bounds check) and walk pixels bound-free.
                    let band = &src[(row + sx0) * N..(row + sx1) * N];
                    for pixel in band.as_chunks::<N>().0 {
                        for (acc, &value) in sum.iter_mut().zip(pixel) {
                            *acc += value.widen();
                        }
                    }
                }

                let count = ((sy1 - sy0) * (sx1 - sx0)) as u64;
                let mean = sum.map(|sum| T::to_u8(sum, count));
                *out = match *mean.as_slice() {
                    [mono] => [mono, mono, mono, u8::MAX],
                    [r, g, b] => [r, g, b, u8::MAX],
                    [r, g, b, a] => [r, g, b, a],
                    _ => unreachable!("one, three or four channels"),
                };
            }
        });

    out
}

#[cfg(test)]
mod tests;
