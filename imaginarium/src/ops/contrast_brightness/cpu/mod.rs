mod affine_row;

use common::par;
use rayon::prelude::*;
use simd::IsaTier;

use crate::color::color_format::{ColorFormat, SampleType};
use crate::interleaved::interleaved_image::InterleavedImage;
use crate::ops::contrast_brightness::ContrastBrightness;
use crate::ops::contrast_brightness::channel_affine::ChannelAffine;
use crate::ops::contrast_brightness::cpu::affine_row::{AffineRow, Alpha};

/// The vector kernel of a format, picked once per image: every format has one, by sample type,
/// and an RGBA one keeps its alpha.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct AffineKernel {
    sample_type: SampleType,
    alpha: Alpha,
}

impl AffineKernel {
    pub(super) const fn of(format: ColorFormat) -> Self {
        Self {
            sample_type: format.sample_type,
            alpha: if format.has_alpha() {
                Alpha::Kept
            } else {
                Alpha::Absent
            },
        }
    }

    /// One packed row of the kernel's format adjusted in place, on `tier`.
    #[inline]
    pub(super) fn adjust_row(self, tier: IsaTier, affine: ChannelAffine, row: &mut [u8]) {
        match self.sample_type {
            SampleType::U8 => tier.run(AffineRow::<u8>::new(affine, self.alpha, row)),
            SampleType::U16 => tier.run(AffineRow::<u16>::new(affine, self.alpha, row)),
            SampleType::F32 => tier.run(AffineRow::<f32>::new(affine, self.alpha, row)),
        }
    }
}

/// Applies contrast and brightness adjustment to an image in place using CPU.
/// The kernels read and write the same row, so the path holds no scratch buffer
/// and allocates nothing.
pub(super) fn apply(params: ContrastBrightness, image: &mut InterleavedImage) {
    apply_on(IsaTier::widest(), params, image);
}

/// [`apply`] on `tier`, one rayon job per row. Rows carry no padding, so coarser jobs would be
/// correct too, but they measure slower: on a hybrid core CPU the small jobs are what lets
/// work-stealing keep the efficiency cores from holding up a frame.
fn apply_on(tier: IsaTier, params: ContrastBrightness, image: &mut InterleavedImage) {
    let format = image.desc().color_format();
    let affine = ChannelAffine::new(params, format);
    let kernel = AffineKernel::of(format);
    let stride = image.desc().row_byte_len();

    par::chunks_mut(image.bytes_mut(), stride).for_each(|row| kernel.adjust_row(tier, affine, row));
}

#[cfg(feature = "internals")]
pub(crate) mod internals {
    use crate::color::color_format::{ColorFormat, SampleType};
    use crate::color::sample::Sample;
    use crate::ops::contrast_brightness::channel_affine::ChannelAffine;

    /// The scalar reference over one packed row of `format` in place, through
    /// [`ChannelAffine::apply`], which the tests hold every kernel to.
    pub(crate) fn scalar_row(affine: ChannelAffine, format: ColorFormat, row: &mut [u8]) {
        match format.sample_type {
            SampleType::U8 => typed_row::<u8>(affine, format, row),
            SampleType::U16 => typed_row::<u16>(affine, format, row),
            SampleType::F32 => typed_row::<f32>(affine, format, row),
        }
    }

    fn typed_row<T: Sample>(affine: ChannelAffine, format: ColorFormat, row: &mut [u8]) {
        let row: &mut [T] = bytemuck::cast_slice_mut(row);
        if format.has_alpha() {
            affine.apply_rgba(row);
        } else {
            affine.apply_all(row);
        }
    }
}

#[cfg(test)]
mod tests;
