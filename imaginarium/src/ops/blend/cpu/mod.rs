mod blend_row;

use common::par;
use rayon::prelude::*;
use simd::IsaTier;

use crate::color::color_format::{ColorFormat, SampleType};
use crate::interleaved::interleaved_image::InterleavedImage;
use crate::ops::blend::Blend;
use crate::ops::blend::cpu::blend_row::{Alpha, BlendRow};

/// The vector kernel of a format, picked once per image: every format has one, by sample type,
/// and an RGBA one mixes its alpha alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct BlendKernel {
    sample_type: SampleType,
    alpha: Alpha,
}

impl BlendKernel {
    pub(super) const fn of(format: ColorFormat) -> Self {
        Self {
            sample_type: format.sample_type,
            alpha: if format.has_alpha() {
                Alpha::Mixed
            } else {
                Alpha::Absent
            },
        }
    }

    /// One row pair of the kernel's format blended into `out`, on `tier`.
    #[inline]
    pub(super) fn blend_row(
        self,
        tier: IsaTier,
        params: Blend,
        src: &[u8],
        dst: &[u8],
        out: &mut [u8],
    ) {
        match self.sample_type {
            SampleType::U8 => tier.run(BlendRow::<u8>::new(params, self.alpha, src, dst, out)),
            SampleType::U16 => tier.run(BlendRow::<u16>::new(params, self.alpha, src, dst, out)),
            SampleType::F32 => tier.run(BlendRow::<f32>::new(params, self.alpha, src, dst, out)),
        }
    }
}

/// Blends `src` over `dst` into `output`, all three sharing a descriptor.
pub(super) fn apply(
    params: Blend,
    src: &InterleavedImage,
    dst: &InterleavedImage,
    output: &mut InterleavedImage,
) {
    apply_on(IsaTier::widest(), params, src, dst, output);
}

/// [`apply`] on `tier`, one rayon job per row.
fn apply_on(
    tier: IsaTier,
    params: Blend,
    src: &InterleavedImage,
    dst: &InterleavedImage,
    output: &mut InterleavedImage,
) {
    src.desc().assert_same(dst.desc(), "src/dst");
    src.desc().assert_same(output.desc(), "src/output");

    let kernel = BlendKernel::of(src.desc().color_format());
    let stride = src.desc().row_byte_len();
    let (src_bytes, dst_bytes) = (src.bytes(), dst.bytes());

    par::chunks_mut(output.bytes_mut(), stride)
        .enumerate()
        .for_each(|(y, out_row)| {
            let src_row = &src_bytes[y * stride..][..stride];
            let dst_row = &dst_bytes[y * stride..][..stride];
            kernel.blend_row(tier, params, src_row, dst_row, out_row);
        });
}

#[cfg(feature = "internals")]
pub(crate) mod internals {
    use crate::color::color_format::{ColorFormat, SampleType};
    use crate::color::sample::Sample;
    use crate::ops::blend::{Blend, BlendMode};

    /// One channel value blended against its destination: both normalized, blended, scaled back
    /// and clamped to the full scale, then narrowed by the crate's rounding rule. A NaN from a
    /// float channel or alpha survives the clamp, as `f32::clamp` leaves it.
    #[inline]
    fn blend_value<T: Sample>(src: T, dst: T, mode: BlendMode, alpha: f32) -> T {
        let blended = mode.blend(src.to_unit(), dst.to_unit(), alpha);
        T::from_f32((blended * T::FULL_SCALE_F32).clamp(0.0, T::FULL_SCALE_F32))
    }

    /// Blends one pixel in place: `params.mode` over the leading `color_count` channels,
    /// and — where the format carries alpha, so `color_count` is one short of the pixel —
    /// a plain alpha mix over the last one, which carries no mode of its own.
    #[inline]
    fn blend_pixel<T: Sample>(
        params: Blend,
        src: &[T],
        dst: &[T],
        out: &mut [T],
        color_count: usize,
    ) {
        let Blend { mode, alpha } = params;
        for ((&s, &d), o) in src[..color_count]
            .iter()
            .zip(&dst[..color_count])
            .zip(out[..color_count].iter_mut())
        {
            *o = blend_value(s, d, mode, alpha);
        }
        for ((&s, &d), o) in src[color_count..]
            .iter()
            .zip(&dst[color_count..])
            .zip(out[color_count..].iter_mut())
        {
            *o = blend_value(s, d, BlendMode::Normal, alpha);
        }
    }

    /// The scalar reference over one packed row pair of `format`: per-channel
    /// blending through [`blend_value`], which the tests hold every kernel to.
    pub(crate) fn scalar_row(
        params: Blend,
        format: ColorFormat,
        src: &[u8],
        dst: &[u8],
        out: &mut [u8],
    ) {
        match format.sample_type {
            SampleType::U8 => typed_row::<u8>(params, format, src, dst, out),
            SampleType::U16 => typed_row::<u16>(params, format, src, dst, out),
            SampleType::F32 => typed_row::<f32>(params, format, src, dst, out),
        }
    }

    fn typed_row<T: Sample>(
        params: Blend,
        format: ColorFormat,
        src: &[u8],
        dst: &[u8],
        out: &mut [u8],
    ) {
        let channel_count = format.channel_count.count();
        // Channels the blend mode applies to; alpha, where the format has one, is
        // the last channel and is left to the mix alone.
        let color_count = channel_count - usize::from(format.has_alpha());
        let src: &[T] = bytemuck::cast_slice(src);
        let dst: &[T] = bytemuck::cast_slice(dst);
        let out: &mut [T] = bytemuck::cast_slice_mut(out);

        let inputs = src
            .chunks_exact(channel_count)
            .zip(dst.chunks_exact(channel_count));
        for ((src_px, dst_px), out_px) in inputs.zip(out.chunks_exact_mut(channel_count)) {
            blend_pixel(params, src_px, dst_px, out_px, color_count);
        }
    }
}

#[cfg(test)]
mod tests;
