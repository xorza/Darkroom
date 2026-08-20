#[cfg(feature = "bench")]
pub(crate) mod bench;
mod cpu;
#[cfg(feature = "wgpu")]
mod gpu;
#[cfg(feature = "wgpu")]
pub(crate) mod pipeline;

#[cfg(feature = "wgpu")]
use crate::common::error::Result;
#[cfg(feature = "wgpu")]
use crate::gpu::Gpu;
#[cfg(feature = "wgpu")]
use crate::gpu::gpu_image::GpuImage;
use crate::image::Image;
#[cfg(feature = "wgpu")]
use crate::ops::contrast_brightness::pipeline::GpuContrastBrightnessPipeline;

/// Parameters for contrast and brightness adjustment.
#[derive(Debug, Clone, Copy)]
pub struct ContrastBrightness {
    /// Contrast multiplier. 1.0 = no change, >1.0 = more contrast, <1.0 = less contrast.
    pub contrast: f32,
    /// Brightness offset in normalized range [-1.0, 1.0].
    /// Positive values brighten, negative values darken.
    pub brightness: f32,
}

impl Default for ContrastBrightness {
    fn default() -> Self {
        Self {
            contrast: 1.0,
            brightness: 0.0,
        }
    }
}

impl ContrastBrightness {
    pub fn new(contrast: f32, brightness: f32) -> Self {
        Self {
            contrast,
            brightness,
        }
    }

    /// Builder method to set contrast.
    pub fn contrast(mut self, contrast: f32) -> Self {
        self.contrast = contrast;
        self
    }

    /// Builder method to set brightness.
    pub fn brightness(mut self, brightness: f32) -> Self {
        self.brightness = brightness;
        self
    }

    /// Applies contrast and brightness adjustment to an image **in place** using CPU,
    /// so an owning caller pays no output allocation.
    ///
    /// The formula applied to each color channel is:
    /// `output = (input - mid) * contrast + mid + brightness`
    ///
    /// Where `mid` is the middle value of the type's range.
    /// Alpha channel (if present) is preserved unchanged.
    pub fn apply_cpu(&self, image: &mut Image) {
        cpu::apply(self, image);
    }

    /// Applies contrast and brightness adjustment using GPU.
    ///
    /// # Panics
    /// Panics if images have different dimensions or color formats.
    #[cfg(feature = "wgpu")]
    pub fn apply_gpu(
        &self,
        ctx: &Gpu,
        pipeline: &GpuContrastBrightnessPipeline,
        input: &GpuImage,
        output: &mut GpuImage,
    ) -> Result<()> {
        gpu::apply(self, ctx, pipeline, input, output)
    }
}

#[cfg(test)]
mod tests {
    use crate::common::color_format::ALL_FORMATS;
    use crate::common::image_diff::pixels_equal;
    use crate::common::internals::create_test_image;
    use crate::ops::contrast_brightness::ContrastBrightness;

    const OP: ContrastBrightness = ContrastBrightness {
        contrast: 1.5,
        brightness: 0.1,
    };

    #[test]
    fn apply_cpu_adjusts_in_place_without_reallocating() {
        for format in ALL_FORMATS {
            let source = create_test_image(*format, 17, 5, 0);
            let mut image = source.clone();
            let before = image.bytes().as_ptr();

            OP.apply_cpu(&mut image);

            assert_eq!(
                before,
                image.bytes().as_ptr(),
                "{format}: apply_cpu should have written the existing storage, not replaced it"
            );
            assert!(
                !pixels_equal(&source, &image),
                "{format}: apply_cpu left the image unchanged"
            );
        }
    }
}
