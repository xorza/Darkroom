mod gather_bilinear;
mod packed_source;
mod source_map;

use common::par;
use rayon::prelude::*;
use simd::IsaTier;

use crate::color::color_format::{ChannelCount, SampleType};
use crate::color::sample::Sample;
use crate::interleaved::interleaved_desc::InterleavedDesc;
use crate::interleaved::interleaved_image::InterleavedImage;
use crate::ops::affine_warp::cpu::gather_bilinear::GatherBilinear;
use crate::ops::affine_warp::cpu::packed_source::PackedSource;
use crate::ops::affine_warp::cpu::source_map::SourceMap;
use crate::ops::affine_warp::{AffineWarp, FilterMode};

/// The vector kernel of a warp, picked once per image: bilinear RGB and RGBA. Every other warp
/// takes the scalar reference: nearest is a copy of one tap, and a mono pixel fills one lane of
/// a gather's four bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct BilinearKernel {
    sample_type: SampleType,
    channels: ChannelCount,
}

impl BilinearKernel {
    /// The kernel of `warp` from an image of `input` to one of `output`, or `None` when it has
    /// none. The gathers index a tap's bytes in `i32`, and the float bounds hold every input
    /// edge and output centre exactly, so an image past those takes the reference too.
    pub(super) fn of(
        warp: &AffineWarp,
        input: InterleavedDesc,
        output: InterleavedDesc,
    ) -> Option<Self> {
        let format = input.color_format();
        let fits = input.size().pixel_count() <= i32::MAX as usize / 16
            && input.size().width <= 1 << 24
            && input.size().height <= 1 << 24
            && output.size().width <= 1 << 23;
        (warp.filter == FilterMode::Bilinear && format.channel_count != ChannelCount::Mono && fits)
            .then_some(Self {
                sample_type: format.sample_type,
                channels: format.channel_count,
            })
    }

    /// Output row `y` of a warp by `map` from `input`, on `tier`.
    #[inline]
    fn warp_row(
        self,
        tier: IsaTier,
        input: &InterleavedImage,
        map: SourceMap,
        y: usize,
        row: &mut [u8],
    ) {
        macro_rules! gather {
            ($sample:ty, $channels:literal) => {
                tier.run(GatherBilinear::<$sample, $channels> {
                    source: PackedSource::of(input),
                    map,
                    y,
                    row: bytemuck::cast_slice_mut(row),
                })
            };
        }
        match (self.sample_type, self.channels) {
            (SampleType::U8, ChannelCount::Rgb) => gather!(u8, 3),
            (SampleType::U8, ChannelCount::Rgba) => gather!(u8, 4),
            (SampleType::U16, ChannelCount::Rgb) => gather!(u16, 3),
            (SampleType::U16, ChannelCount::Rgba) => gather!(u16, 4),
            (SampleType::F32, ChannelCount::Rgb) => gather!(f32, 3),
            (SampleType::F32, ChannelCount::Rgba) => gather!(f32, 4),
            (_, ChannelCount::Mono) => unreachable!("a mono warp has no kernel"),
        }
    }
}

/// Applies an affine transform to `input`, sampling into `output`.
///
/// Output dimensions come from `output`'s descriptor (they may differ from the
/// input's). Each output pixel center is mapped back through the inverse
/// transform and sampled from the input; sources outside the input read as
/// zero. This mirrors the GPU shader (`shader.wgsl`) so the two backends agree.
///
/// # Panics
/// Panics unless `input` and `output` share a color format. Their dimensions
/// need not match — that is what makes this a resample.
pub(super) fn apply(warp: &AffineWarp, input: &InterleavedImage, output: &mut InterleavedImage) {
    apply_on(IsaTier::widest(), warp, input, output);
}

/// [`apply`] through the warp's kernel on `tier`, or the scalar reference for a warp with none,
/// one rayon job per row.
pub(super) fn apply_on(
    tier: IsaTier,
    warp: &AffineWarp,
    input: &InterleavedImage,
    output: &mut InterleavedImage,
) {
    let format = input.desc().color_format();
    assert_eq!(
        format,
        output.desc().color_format(),
        "input/output color format mismatch"
    );
    let Some(kernel) = BilinearKernel::of(warp, input.desc(), output.desc()) else {
        apply_scalar(warp, input, output);
        return;
    };
    let map = SourceMap::new(warp.inverse());
    let stride = output.desc().row_byte_len();
    par::chunks_mut(output.bytes_mut(), stride)
        .enumerate()
        .for_each(|(y, row)| kernel.warp_row(tier, input, map, y, row));
}

/// The scalar reference, picking the storage type and channel count the format
/// stores pixels in: what a warp with no kernel takes, and what the tests hold
/// every kernel to.
pub(super) fn apply_scalar(
    warp: &AffineWarp,
    input: &InterleavedImage,
    output: &mut InterleavedImage,
) {
    let format = input.desc().color_format();
    match (format.sample_type, format.channel_count) {
        (SampleType::U8, ChannelCount::Mono) => apply_typed::<u8, 1>(warp, input, output),
        (SampleType::U8, ChannelCount::Rgb) => apply_typed::<u8, 3>(warp, input, output),
        (SampleType::U8, ChannelCount::Rgba) => apply_typed::<u8, 4>(warp, input, output),
        (SampleType::U16, ChannelCount::Mono) => apply_typed::<u16, 1>(warp, input, output),
        (SampleType::U16, ChannelCount::Rgb) => apply_typed::<u16, 3>(warp, input, output),
        (SampleType::U16, ChannelCount::Rgba) => apply_typed::<u16, 4>(warp, input, output),
        (SampleType::F32, ChannelCount::Mono) => apply_typed::<f32, 1>(warp, input, output),
        (SampleType::F32, ChannelCount::Rgb) => apply_typed::<f32, 3>(warp, input, output),
        (SampleType::F32, ChannelCount::Rgba) => apply_typed::<f32, 4>(warp, input, output),
    }
}

/// Channel values interpolate in their **native** range (u8 `0..=255`, u16
/// `0..=65535`, f32 unchanged), as the GPU shader does, and narrow back by
/// [`Sample::from_f32`], which rounds to nearest with ties to even — the rule WGSL
/// `round` follows on the GPU. Interpolation is linear, so normalizing to `[0, 1]`
/// first would only add a divide and a multiply per channel per tap, and a second
/// rounding. Float output is written unclamped.
fn apply_typed<T, const N: usize>(
    warp: &AffineWarp,
    input: &InterleavedImage,
    output: &mut InterleavedImage,
) where
    T: Sample,
{
    let source = PackedSource::<T>::of(input);
    let out_w = output.desc().size().width;
    let out_stride = output.desc().row_byte_len();

    let map = SourceMap::new(warp.inverse());
    let filter = warp.filter;

    par::chunks_mut(output.bytes_mut(), out_stride)
        .enumerate()
        .for_each(|(y, out_bytes)| {
            let out_row: &mut [T] = bytemuck::cast_slice_mut(out_bytes);
            for x in 0..out_w {
                let src = map.position(x, y);
                let rgba = match filter {
                    FilterMode::Nearest => source.nearest::<N>(src),
                    FilterMode::Bilinear => source.bilinear::<N>(src),
                };
                write_pixel::<T, N>(&mut out_row[x * N..x * N + N], rgba);
            }
        });
}

/// Writes the low `N` lanes back into the output pixel's channels.
#[inline]
fn write_pixel<T, const N: usize>(out: &mut [T], rgba: [f32; 4])
where
    T: Sample,
{
    for (dst, &v) in out.iter_mut().zip(rgba.iter()) {
        *dst = T::from_f32(v);
    }
}

#[cfg(test)]
mod tests;
