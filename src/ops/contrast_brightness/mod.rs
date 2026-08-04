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

    /// Applies the operation, automatically choosing CPU or GPU based on data location.
    ///
    /// Prefers GPU if any input/output is already on GPU and the format is supported.
    ///
    /// # Errors
    /// Returns an error if:
    /// - Input and output have different color formats
    /// - The color format is not supported by either CPU or GPU implementation
    pub fn execute(
        &self,
        ctx: &mut ProcessingContext,
        input: &ImageBuffer,
        output: &mut ImageBuffer,
    ) -> Result<()> {
        let backend = select_backend(
            ctx,
            &[input, output],
            ALL_FORMATS,
            ALL_FORMATS,
            "ContrastBrightness",
        )?;

        match backend {
            #[cfg(feature = "wgpu")]
            Backend::Gpu => self.execute_gpu(ctx, input, output),
            Backend::Cpu => self.execute_cpu(ctx, input, output),
        }
    }

    /// Applies the operation using CPU with ImageBuffer.
    ///
    /// Automatically downloads images from GPU if needed. The op itself is in-place
    /// ([`apply_cpu`](Self::apply_cpu)); this buffer-level entry point clones the
    /// input into a fresh image first, preserving `execute`'s out-of-place contract.
    /// `output` is replaced wholesale (any prior storage and descriptor dropped).
    pub fn execute_cpu(
        &self,
        ctx: &mut ProcessingContext,
        input: &ImageBuffer,
        output: &mut ImageBuffer,
    ) -> Result<()> {
        let mut image = input.make_cpu(ctx)?.clone();
        self.apply_cpu(&mut image);
        *output = ImageBuffer::from_cpu(image);
        Ok(())
    }
}
