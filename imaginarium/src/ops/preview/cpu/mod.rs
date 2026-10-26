use std::cmp::Ordering;
use std::ops::{AddAssign, Range};

use common::{Size2us, par};
use rayon::prelude::*;

use crate::color::color_format::{ChannelCount, ColorFormat, SampleType};
use crate::color::sample::Sample;
use crate::interleaved::interleaved_desc::InterleavedDesc;
use crate::interleaved::interleaved_image::InterleavedImage;
use crate::ops::preview::Preview;
use crate::planar::planar_image::PlanarImage;

/// Area-averages `input` to `params`' target size, fused with the conversion to
/// `RGBA_U8`. See [`Preview`] for the algorithm.
pub(super) fn generate(params: &Preview, input: &InterleavedImage) -> InterleavedImage {
    let target = target(params);
    let format = input.desc().color_format();
    match (format.sample_type, format.channel_count) {
        (SampleType::U8, ChannelCount::Mono) => typed::<u8, 1>(input, target),
        (SampleType::U8, ChannelCount::Rgb) => typed::<u8, 3>(input, target),
        (SampleType::U8, ChannelCount::Rgba) => typed::<u8, 4>(input, target),
        (SampleType::U16, ChannelCount::Mono) => typed::<u16, 1>(input, target),
        (SampleType::U16, ChannelCount::Rgb) => typed::<u16, 3>(input, target),
        (SampleType::U16, ChannelCount::Rgba) => typed::<u16, 4>(input, target),
        (SampleType::F32, ChannelCount::Mono) => typed::<f32, 1>(input, target),
        (SampleType::F32, ChannelCount::Rgb) => typed::<f32, 3>(input, target),
        (SampleType::F32, ChannelCount::Rgba) => typed::<f32, 4>(input, target),
    }
}

/// [`generate`] of `f32` planes, each footprint's mean of each plane as the `f32` interleaved
/// path takes it.
pub(super) fn generate_planar(params: &Preview, input: &PlanarImage) -> InterleavedImage {
    let target = target(params);
    let size = input.desc().size();
    match input.channel_count() {
        1 => planar([input.channel(0).pixels()], size, target),
        _ => planar(
            [0, 1, 2].map(|channel| input.channel(channel).pixels()),
            size,
            target,
        ),
    }
}

/// The preview's size, at least one pixel on each side.
fn target(params: &Preview) -> Size2us {
    Size2us::new(params.size.width.max(1), params.size.height.max(1))
}

/// A source sample type: the sum a footprint accumulates in, and how a footprint's sum turns
/// into an output byte.
///
/// Integer samples sum exactly in `u64` and narrow by an exact rational rounding; `f32` sums in
/// `f64`. Either way the byte is the footprint mean narrowed as [`Sample::convert`] narrows a
/// single value — so a one-pixel footprint is exactly the format conversion.
trait PreviewSample: Sample {
    type Sum: Copy + Default + AddAssign + PartialEq;
    fn widen(self) -> Self::Sum;
    /// This sample weighted by `alpha`, exactly: a `u16` product fits a `u64`, an `f32` one an
    /// `f64`.
    fn weighted(self, alpha: Self) -> Self::Sum;
    /// The byte of the mean `sum / count`, `count` in the sum's units.
    fn to_u8(sum: Self::Sum, count: Self::Sum) -> u8;
    fn count(count: u64) -> Self::Sum;
}

impl PreviewSample for u8 {
    type Sum = u64;

    #[inline]
    fn widen(self) -> u64 {
        u64::from(self)
    }

    #[inline]
    fn weighted(self, alpha: Self) -> u64 {
        u64::from(self) * u64::from(alpha)
    }

    fn to_u8(sum: u64, count: u64) -> u8 {
        narrow(round_div(sum, count))
    }

    fn count(count: u64) -> u64 {
        count
    }
}

impl PreviewSample for u16 {
    type Sum = u64;

    #[inline]
    fn widen(self) -> u64 {
        u64::from(self)
    }

    #[inline]
    fn weighted(self, alpha: Self) -> u64 {
        u64::from(self) * u64::from(alpha)
    }

    /// `mean · 255 / 65535` is `sum / (257 · count)`.
    fn to_u8(sum: u64, count: u64) -> u8 {
        narrow(round_div(sum, 257 * count))
    }

    fn count(count: u64) -> u64 {
        count
    }
}

impl PreviewSample for f32 {
    type Sum = f64;

    #[inline]
    fn widen(self) -> f64 {
        f64::from(self)
    }

    #[inline]
    fn weighted(self, alpha: Self) -> f64 {
        f64::from(self) * f64::from(alpha)
    }

    #[expect(
        clippy::cast_sign_loss,
        reason = "the saturating `as` cast is the narrowing rule: negatives and NaN to zero, overflow to the maximum"
    )]
    fn to_u8(sum: f64, count: f64) -> u8 {
        (sum / count * 255.0).round_ties_even() as u8
    }

    fn count(count: u64) -> f64 {
        count as f64
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

/// The footprint of output index `out` of `dst` over a source axis of `src`: the source indices
/// whose integer floor maps to it, at least one, so an upscale samples its nearest instead.
fn footprint(out: usize, src: usize, dst: usize) -> Range<usize> {
    let start = out * src / dst;
    let end = ((out + 1) * src / dst).max(start + 1).min(src);
    start..end
}

/// The `RGBA_U8` preview of a `src`-sized source at `target`, each pixel `mean` of its footprint's
/// rows and columns, row-parallel.
fn fill(
    src: Size2us,
    target: Size2us,
    mean: impl Fn(Range<usize>, Range<usize>) -> [u8; 4] + Sync,
) -> InterleavedImage {
    let mut out = InterleavedImage::new_zeroed(InterleavedDesc::new(target, ColorFormat::RGBA_U8));
    par::chunks_mut(out.bytes_mut(), target.width * 4)
        .enumerate()
        .for_each(|(oy, out_row)| {
            let rows = footprint(oy, src.height, target.height);
            for (ox, out) in out_row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                *out = mean(rows.clone(), footprint(ox, src.width, target.width));
            }
        });
    out
}

/// One output pixel of `N` channel means: mono broadcast, an opaque alpha where the source has
/// none.
fn rgba<const N: usize>(mean: [u8; N]) -> [u8; 4] {
    match *mean.as_slice() {
        [mono] => [mono, mono, mono, u8::MAX],
        [r, g, b] => [r, g, b, u8::MAX],
        [r, g, b, a] => [r, g, b, a],
        _ => unreachable!("one, three or four channels"),
    }
}

/// The preview of an interleaved image of `N` channels of `T`. With alpha, each color's mean
/// weighs its pixels by their alpha, `Σ c·a / Σ a`, so a transparent pixel lends a footprint no
/// color; a footprint of no alpha at all, which has no color to weigh, takes the plain mean.
fn typed<T: PreviewSample, const N: usize>(
    input: &InterleavedImage,
    target: Size2us,
) -> InterleavedImage {
    let src_size = input.desc().size();
    let src: &[T] = bytemuck::cast_slice(input.bytes());
    fill(src_size, target, |rows, cols| {
        let mut sum = [T::Sum::default(); N];
        let mut weighted = [T::Sum::default(); 3];
        for sy in rows.clone() {
            let row = sy * src_size.width;
            // The footprint's columns are contiguous in the row; slice the band once (one bounds
            // check) and walk pixels bound-free.
            let band = &src[(row + cols.start) * N..(row + cols.end) * N];
            for pixel in band.as_chunks::<N>().0 {
                for (acc, &value) in sum.iter_mut().zip(pixel) {
                    *acc += value.widen();
                }
                if N == 4 {
                    for (acc, &value) in weighted.iter_mut().zip(pixel) {
                        *acc += value.weighted(pixel[3]);
                    }
                }
            }
        }
        let count = T::count((rows.len() * cols.len()) as u64);
        let mut mean = sum.map(|sum| T::to_u8(sum, count));
        if N == 4 && sum[3] != T::Sum::default() {
            for (mean, weighted) in mean.iter_mut().zip(weighted) {
                *mean = T::to_u8(weighted, sum[3]);
            }
        }
        rgba(mean)
    })
}

/// The preview of `N` `f32` planes of `size`.
fn planar<const N: usize>(planes: [&[f32]; N], size: Size2us, target: Size2us) -> InterleavedImage {
    fill(size, target, |rows, cols| {
        let mut sum = [0.0f64; N];
        for sy in rows.clone() {
            let row = sy * size.width;
            for (acc, plane) in sum.iter_mut().zip(planes) {
                for &value in &plane[row + cols.start..row + cols.end] {
                    *acc += f64::from(value);
                }
            }
        }
        let count = (rows.len() * cols.len()) as f64;
        rgba(sum.map(|sum| f32::to_u8(sum, count)))
    })
}

#[cfg(test)]
mod tests;
