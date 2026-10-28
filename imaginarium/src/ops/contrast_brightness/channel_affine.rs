use crate::color::color_format::{ColorFormat, SampleType};
use crate::color::sample::Sample;
use crate::ops::contrast_brightness::ContrastBrightness;

/// The per-channel affine `value * scale + offset` that contrast/brightness reduces to in a
/// storage type's own units, held to `[low, high]`: an integer store then saturates it to
/// `[0, max]`, and `f32` keeps it.
///
/// Folding brightness and the mid-point recentering into one offset is what
/// lets a row be a single multiply-add. The scalar reference and every vector
/// kernel evaluate exactly this expression in exactly this order, so they agree
/// bit for bit rather than within a rounding tolerance; the GPU takes the scale,
/// offset and max as its uniforms, and the clamp by its shader code in place of
/// `low` and `high`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ChannelAffine {
    pub(crate) scale: f32,
    pub(crate) offset: f32,
    pub(crate) max: f32,
    /// The [`Clamp`]'s bounds in the storage type's units, `±∞` where it holds no end.
    ///
    /// [`Clamp`]: crate::ops::clamp::Clamp
    pub(crate) low: f32,
    pub(crate) high: f32,
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
            low: params.clamp.low(),
            high: params.clamp.high(max),
        }
    }
}

#[cfg(feature = "internals")]
pub(crate) mod internals {
    use crate::color::sample::Sample;
    use crate::ops::clamp::internals::hold;
    use crate::ops::contrast_brightness::channel_affine::ChannelAffine;

    /// The scalar reference, which the tests hold every kernel to.
    impl ChannelAffine {
        /// One channel value adjusted: the affine, [`hold`] to `[low, high]`, then narrowed by the
        /// crate's rounding rule, which saturates an integer to `[0, max]` and keeps an `f32`.
        #[inline]
        pub(crate) fn apply<T: Sample>(self, value: T) -> T {
            let value = value.to_f32() * self.scale + self.offset;
            T::from_f32(hold(value, self.low, self.high))
        }

        /// Every channel value of `values` adjusted in place: the scalar reference over a row with
        /// no alpha.
        #[inline]
        pub(crate) fn apply_all<T: Sample>(self, values: &mut [T]) {
            for value in values {
                *value = self.apply(*value);
            }
        }

        /// The color channels of whole RGBA pixels adjusted in place, alpha left as it is: the
        /// scalar reference over an RGBA row.
        #[inline]
        pub(crate) fn apply_rgba<T: Sample>(self, pixels: &mut [T]) {
            let (pixels, rest) = pixels.as_chunks_mut::<4>();
            debug_assert!(rest.is_empty(), "a whole number of RGBA pixels");
            for pixel in pixels {
                self.apply_all(&mut pixel[..3]);
            }
        }
    }
}
