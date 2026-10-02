use crate::common::color_format::{ColorFormat, SampleType};
use crate::common::sample::Sample;
use crate::ops::contrast_brightness::ContrastBrightness;

/// The per-channel affine `value * scale + offset`, clamped to `[0, max]`, that
/// contrast/brightness reduces to in a storage type's own units.
///
/// Folding brightness and the mid-point recentering into one offset is what
/// lets a row be a single multiply-add. The scalar reference and every SIMD
/// kernel evaluate exactly this expression in exactly this order, so their
/// integer results agree bit for bit rather than within a rounding tolerance;
/// the GPU takes these values as its uniforms.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ChannelAffine {
    pub(crate) scale: f32,
    pub(crate) offset: f32,
    pub(crate) max: f32,
}

impl ChannelAffine {
    pub(crate) fn new(params: ContrastBrightness, format: ColorFormat) -> Self {
        let max = match format.sample_type {
            SampleType::U8 => u8::FULL_SCALE_F32,
            SampleType::U16 => u16::FULL_SCALE_F32,
            SampleType::F32 => f32::FULL_SCALE_F32,
        };
        let mid = max / 2.0;
        Self {
            scale: params.contrast,
            offset: mid * (1.0 - params.contrast) + params.brightness * max,
            max,
        }
    }

    /// One channel value adjusted: the affine, clamped to `[0, max]`, then narrowed by the
    /// crate's rounding rule. A NaN survives the clamp, as `f32::clamp` leaves it.
    #[inline]
    pub(crate) fn apply<T: Sample>(self, value: T) -> T {
        T::from_f32((value.to_f32() * self.scale + self.offset).clamp(0.0, self.max))
    }
}
