mod channel_affine;
mod cpu;
#[cfg(feature = "wgpu")]
mod gpu;
#[cfg(feature = "wgpu")]
mod pipeline;

#[cfg(feature = "wgpu")]
use crate::gpu::Gpu;
#[cfg(feature = "wgpu")]
use crate::gpu::gpu_image::GpuImage;
use crate::interleaved::interleaved_image::InterleavedImage;
use crate::ops::clamp::Clamp;

/// Parameters for contrast and brightness adjustment.
#[derive(Debug, Clone, Copy)]
pub struct ContrastBrightness {
    /// Contrast multiplier. 1.0 = no change, >1.0 = more contrast, <1.0 = less contrast.
    pub contrast: f32,
    /// Brightness offset in normalized range [-1.0, 1.0].
    /// Positive values brighten, negative values darken.
    pub brightness: f32,
    /// The ends of the range an `f32` result is held to; an integer one saturates to its range
    /// whatever this is.
    pub clamp: Clamp,
}

impl Default for ContrastBrightness {
    fn default() -> Self {
        Self::new(1.0, 0.0)
    }
}

impl ContrastBrightness {
    /// `contrast` and `brightness` under [`Clamp::DEFAULT`].
    pub const fn new(contrast: f32, brightness: f32) -> Self {
        Self {
            contrast,
            brightness,
            clamp: Clamp::DEFAULT,
        }
    }

    /// Builder method to set contrast.
    #[must_use]
    pub const fn contrast(mut self, contrast: f32) -> Self {
        self.contrast = contrast;
        self
    }

    /// Builder method to set brightness.
    #[must_use]
    pub const fn brightness(mut self, brightness: f32) -> Self {
        self.brightness = brightness;
        self
    }

    /// Builder method to set the clamp.
    #[must_use]
    pub const fn clamp(mut self, clamp: Clamp) -> Self {
        self.clamp = clamp;
        self
    }

    /// Applies contrast and brightness adjustment to an image **in place** using CPU,
    /// so an owning caller pays no output allocation.
    ///
    /// The formula applied to each color channel is:
    /// `output = (input - mid) * contrast + mid + brightness`, held to the range's ends
    /// [`Self::clamp`] names.
    ///
    /// Where `mid` is the middle value of the type's range.
    /// Alpha channel (if present) is preserved unchanged.
    pub fn apply_cpu(&self, image: &mut InterleavedImage) {
        cpu::apply(*self, image);
    }

    /// Applies contrast and brightness adjustment on the GPU, in place, as [`Self::apply_cpu`]
    /// does.
    #[cfg(feature = "wgpu")]
    pub fn apply_gpu(&self, gpu: &Gpu, image: &mut GpuImage) {
        gpu::apply(*self, gpu, image);
    }
}

#[cfg(feature = "internals")]
pub(crate) mod bench;

#[cfg(test)]
mod tests {
    use common::Size2us;

    use crate::color::color_format::ColorFormat;
    use crate::internals::create_test_image;
    use crate::ops::contrast_brightness::ContrastBrightness;

    const OP: ContrastBrightness = ContrastBrightness::new(1.5, 0.1);

    #[test]
    fn apply_cpu_adjusts_in_place_without_reallocating() {
        for format in ColorFormat::ALL {
            let source = create_test_image(format, Size2us::new(17, 5), 0);
            let mut image = source.clone();
            let before = image.bytes().as_ptr();

            OP.apply_cpu(&mut image);

            assert_eq!(
                before,
                image.bytes().as_ptr(),
                "{format}: apply_cpu should have written the existing storage, not replaced it"
            );
        }
    }
}
