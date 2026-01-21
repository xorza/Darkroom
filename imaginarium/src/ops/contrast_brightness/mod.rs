mod cpu;
mod gpu;
mod pipeline;

use crate::common::color_format::ColorFormat;
use crate::common::error::Result;
use crate::gpu::{Gpu, GpuImage};
use crate::image::Image;
use crate::ops::{Backend, select_backend};
use crate::processing_context::{ImageBuffer, ProcessingContext};

pub use pipeline::GpuContrastBrightnessPipeline;

const SUPPORTED_CPU_FORMATS: &[ColorFormat] = &[
    ColorFormat::GRAY_U8,
    ColorFormat::GRAY_U16,
    ColorFormat::GRAY_F32,
    ColorFormat::GRAY_ALPHA_U8,
    ColorFormat::GRAY_ALPHA_U16,
    ColorFormat::GRAY_ALPHA_F32,
    ColorFormat::RGB_U8,
    ColorFormat::RGB_U16,
    ColorFormat::RGB_F32,
    ColorFormat::RGBA_U8,
    ColorFormat::RGBA_U16,
    ColorFormat::RGBA_F32,
];

const SUPPORTED_GPU_FORMATS: &[ColorFormat] = &[
    ColorFormat::GRAY_U8,
    ColorFormat::GRAY_ALPHA_U8,
    ColorFormat::RGB_U8,
    ColorFormat::RGBA_U8,
    ColorFormat::GRAY_U16,
    ColorFormat::GRAY_ALPHA_U16,
    ColorFormat::RGB_U16,
    ColorFormat::RGBA_U16,
    ColorFormat::GRAY_F32,
    ColorFormat::GRAY_ALPHA_F32,
    ColorFormat::RGB_F32,
    ColorFormat::RGBA_F32,
];

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

    /// Applies contrast and brightness adjustment to an image using CPU.
    ///
    /// The formula applied to each color channel is:
    /// `output = (input - mid) * contrast + mid + brightness`
    ///
    /// Where `mid` is the middle value of the type's range.
    /// Alpha channel (if present) is preserved unchanged.
    ///
    /// # Panics
    /// Panics if input and output images have different dimensions or color formats.
    pub fn apply_cpu(&self, input: &Image, output: &mut Image) {
        cpu::apply(self, input, output);
    }

    /// Applies contrast and brightness adjustment using GPU.
    /// Only supports RGBA_U8 format.
    ///
    /// # Arguments
    /// * `ctx` - The GPU context
    /// * `pipeline` - The cached contrast/brightness pipeline
    /// * `input` - The input image
    /// * `output` - The output image
    ///
    /// # Panics
    /// Panics if images have different dimensions or color formats.
    pub fn apply_gpu(
        &self,
        ctx: &Gpu,
        pipeline: &GpuContrastBrightnessPipeline,
        input: &GpuImage,
        output: &mut GpuImage,
    ) -> Result<()> {
        gpu::apply(self, ctx, pipeline, input, output)
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
            [input, output as &ImageBuffer],
            SUPPORTED_CPU_FORMATS,
            SUPPORTED_GPU_FORMATS,
            "ContrastBrightness",
        )?;

        match backend {
            Backend::Gpu => self.execute_gpu(ctx, input, output),
            Backend::Cpu => self.execute_cpu(ctx, input, output),
        }
    }

    /// Applies the operation using CPU with ImageBuffer.
    ///
    /// Automatically downloads images from GPU if needed.
    pub fn execute_cpu(
        &self,
        ctx: &mut ProcessingContext,
        input: &ImageBuffer,
        output: &mut ImageBuffer,
    ) -> Result<()> {
        let input_cpu = input.make_cpu(ctx)?;
        let mut output_cpu = output.make_cpu_mut(ctx)?;

        self.apply_cpu(&input_cpu, &mut output_cpu);

        Ok(())
    }

    /// Applies the operation using GPU with ImageBuffer.
    ///
    /// Automatically uploads images to GPU if needed.
    pub fn execute_gpu(
        &self,
        ctx: &mut ProcessingContext,
        input: &ImageBuffer,
        output: &mut ImageBuffer,
    ) -> Result<()> {
        let input_gpu = input.make_gpu(ctx)?;
        let mut output_gpu = output.make_gpu_mut(ctx)?;

        let gpu_processing_ctx = ctx
            .gpu_context()
            .expect("GPU context required for contrast/brightness");

        let gpu_ctx = gpu_processing_ctx.gpu().clone();
        let pipeline = gpu_processing_ctx.get_or_create(GpuContrastBrightnessPipeline::new)?;

        self.apply_gpu(&gpu_ctx, pipeline, &input_gpu, &mut output_gpu)
    }
}
