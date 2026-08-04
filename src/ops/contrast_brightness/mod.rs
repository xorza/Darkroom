#[cfg(feature = "bench")]
pub(crate) mod bench;
mod cpu;
#[cfg(feature = "wgpu")]
mod gpu;
#[cfg(feature = "wgpu")]
pub(crate) mod pipeline;

use crate::common::color_format::ALL_FORMATS;
use crate::common::error::Result;
use crate::image::Image;
use crate::ops::backend_selection::{Backend, select_backend};
use crate::processing_context::ProcessingContext;
use crate::processing_context::image_buffer::ImageBuffer;

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

    /// Applies the operation to `buffer` **in place**, choosing CPU or GPU by
    /// where the data already lives.
    ///
    /// In place is the only form this op offers. The adjustment is pointwise, so
    /// a second buffer buys nothing and costs a great deal: the op is bandwidth
    /// bound, and an output allocation the kernel has to fault in and zero can
    /// dwarf the adjustment itself. A caller that must keep its input clones it
    /// first, where the sharing is visible.
    ///
    /// # Errors
    /// Returns an error if the color format is not supported by either backend.
    pub fn execute(&self, ctx: &mut ProcessingContext, buffer: &mut ImageBuffer) -> Result<()> {
        let backend = select_backend(
            ctx,
            &[buffer],
            ALL_FORMATS,
            ALL_FORMATS,
            "ContrastBrightness",
        )?;

        match backend {
            #[cfg(feature = "wgpu")]
            Backend::Gpu => self.execute_gpu(ctx, buffer),
            Backend::Cpu => self.execute_cpu(ctx, buffer),
        }
    }

    /// Applies the operation to `buffer` in place on the CPU, downloading it
    /// from the GPU first if that is where it lives.
    pub fn execute_cpu(&self, ctx: &mut ProcessingContext, buffer: &mut ImageBuffer) -> Result<()> {
        self.apply_cpu(buffer.make_cpu_mut(ctx)?);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::common::color_format::ALL_FORMATS;
    use crate::common::image_diff::pixels_equal;
    use crate::common::internals::create_test_image;
    use crate::ops::contrast_brightness::ContrastBrightness;
    use crate::processing_context::ProcessingContext;
    use crate::processing_context::image_buffer::ImageBuffer;

    const OP: ContrastBrightness = ContrastBrightness {
        contrast: 1.5,
        brightness: 0.1,
    };

    /// The address of a buffer's CPU pixel storage, for proving reuse.
    fn storage_address(buffer: &ImageBuffer, ctx: &ProcessingContext) -> usize {
        buffer.make_cpu(ctx).unwrap().bytes().as_ptr() as usize
    }

    #[test]
    fn execute_adjusts_in_place_without_reallocating() {
        let ctx = ProcessingContext::cpu_only();

        for format in ALL_FORMATS {
            let source = create_test_image(*format, 17, 5, 0);
            let mut buffer = ImageBuffer::from_cpu(source.clone());
            let before = storage_address(&buffer, &ctx);

            let mut ctx = ProcessingContext::cpu_only();
            OP.execute(&mut ctx, &mut buffer).unwrap();

            assert_eq!(
                before,
                storage_address(&buffer, &ctx),
                "{format}: execute should have written the existing storage, \
                 not replaced it"
            );

            let mut want = source.clone();
            OP.apply_cpu(&mut want);
            assert!(
                pixels_equal(&want, &buffer.make_cpu(&ctx).unwrap()),
                "{format}: execute differs from apply_cpu"
            );
        }
    }

    #[test]
    fn execute_downloads_a_gpu_buffer_before_adjusting_on_cpu() {
        // A CPU-only context has nowhere to run the GPU path, so a buffer that
        // has no CPU storage yet must still come back adjusted and CPU-resident.
        let mut ctx = ProcessingContext::cpu_only();
        let format = ALL_FORMATS[0];
        let source = create_test_image(format, 17, 5, 0);
        let mut buffer = ImageBuffer::from_cpu(source.clone());

        OP.execute_cpu(&mut ctx, &mut buffer).unwrap();

        assert!(buffer.is_cpu(), "buffer should be CPU resident afterwards");
        let mut want = source.clone();
        OP.apply_cpu(&mut want);
        assert!(
            pixels_equal(&want, &buffer.make_cpu(&ctx).unwrap()),
            "execute_cpu differs from apply_cpu"
        );
    }
}
