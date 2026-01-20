mod cpu;
mod gpu;
mod pipeline;

use crate::common::color_format::ColorFormat;
use crate::common::error::Result;
use crate::gpu::Gpu;
use crate::gpu::GpuImage;
use crate::image::Image;
use crate::ops::{Backend, select_backend};
use crate::processing_context::{ImageBuffer, ProcessingContext};

pub use pipeline::GpuBlendPipeline;

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
    //
    ColorFormat::GRAY_F32,
    ColorFormat::GRAY_ALPHA_F32,
    ColorFormat::RGB_F32,
    ColorFormat::RGBA_F32,
];

/// Blend modes for combining two images.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum BlendMode {
    /// Normal alpha blending: result = src * alpha + dst * (1 - alpha)
    #[default]
    Normal,
    /// Additive blending: result = src + dst (clamped)
    Add,
    /// Subtractive blending: result = dst - src (clamped)
    Subtract,
    /// Multiply blending: result = src * dst
    Multiply,
    /// Screen blending: result = 1 - (1 - src) * (1 - dst)
    Screen,
    /// Overlay blending: combines Multiply and Screen
    Overlay,
}

/// Parameters for image blending.
#[derive(Debug, Clone, Copy)]
pub struct Blend {
    /// The blend mode to use.
    pub mode: BlendMode,
    /// Alpha value for blending in range [0.0, 1.0].
    /// 0.0 = fully dst, 1.0 = fully src (for Normal mode)
    /// For other modes, this controls the strength of the effect.
    pub alpha: f32,
}

impl Default for Blend {
    fn default() -> Self {
        Self {
            mode: BlendMode::Normal,
            alpha: 1.0,
        }
    }
}

impl Blend {
    pub fn new(mode: BlendMode, alpha: f32) -> Self {
        Self { mode, alpha }
    }

    /// Builder method to set blend mode.
    pub fn mode(mut self, mode: BlendMode) -> Self {
        self.mode = mode;
        self
    }

    /// Builder method to set alpha.
    pub fn alpha(mut self, alpha: f32) -> Self {
        self.alpha = alpha;
        self
    }

    /// Applies blending of two images using CPU.
    ///
    /// # Arguments
    /// * `src` - The source (top) image
    /// * `dst` - The destination (bottom) image
    /// * `output` - The output image
    ///
    /// # Panics
    /// Panics if images have different dimensions or color formats.
    pub fn apply_cpu(&self, src: &Image, dst: &Image, output: &mut Image) {
        cpu::apply(self, src, dst, output);
    }

    /// Applies blending of two images using GPU.
    /// Supports U8 and F32 formats for GRAY, GRAY_ALPHA, RGB, and RGBA.
    ///
    /// # Arguments
    /// * `ctx` - The GPU context
    /// * `pipeline` - The cached blend pipeline
    /// * `src` - The source (top) image
    /// * `dst` - The destination (bottom) image
    /// * `output` - The output image
    ///
    /// # Panics
    /// Panics if images have different dimensions or color formats.
    pub fn apply_gpu(
        &self,
        ctx: &Gpu,
        pipeline: &GpuBlendPipeline,
        src: &GpuImage,
        dst: &GpuImage,
        output: &mut GpuImage,
    ) -> Result<()> {
        gpu::apply(self, ctx, pipeline, src, dst, output)
    }

    /// Applies the operation, automatically choosing CPU or GPU based on data location.
    ///
    /// Prefers GPU if any input/output is already on GPU and the format is supported.
    ///
    /// # Errors
    /// Returns an error if:
    /// - Inputs and output have different color formats
    /// - The color format is not supported by either CPU or GPU implementation
    pub fn execute(
        &self,
        ctx: &mut ProcessingContext,
        src: &ImageBuffer,
        dst: &ImageBuffer,
        output: &mut ImageBuffer,
    ) -> Result<()> {
        let backend = select_backend(
            ctx,
            [src, dst, output as &ImageBuffer],
            SUPPORTED_CPU_FORMATS,
            SUPPORTED_GPU_FORMATS,
            "Blend",
        )?;

        match backend {
            Backend::Gpu => self.execute_gpu(ctx, src, dst, output),
            Backend::Cpu => self.execute_cpu(ctx, src, dst, output),
        }
    }

    /// Applies the operation using CPU with ImageBuffer.
    ///
    /// Automatically downloads images from GPU if needed.
    pub fn execute_cpu(
        &self,
        ctx: &mut ProcessingContext,
        src: &ImageBuffer,
        dst: &ImageBuffer,
        output: &mut ImageBuffer,
    ) -> Result<()> {
        let src_cpu = src.make_cpu(ctx)?;
        let dst_cpu = dst.make_cpu(ctx)?;
        let mut output_cpu = output.make_cpu_mut(ctx)?;

        self.apply_cpu(&src_cpu, &dst_cpu, &mut output_cpu);

        Ok(())
    }

    /// Applies the operation using GPU with ImageBuffer.
    ///
    /// Automatically uploads images to GPU if needed.
    pub fn execute_gpu(
        &self,
        ctx: &mut ProcessingContext,
        src: &ImageBuffer,
        dst: &ImageBuffer,
        output: &mut ImageBuffer,
    ) -> Result<()> {
        let src_gpu = src.make_gpu(ctx)?;
        let dst_gpu = dst.make_gpu(ctx)?;
        let mut output_gpu = output.make_gpu_mut(ctx)?;

        let gpu_processing_ctx = ctx.gpu_context().expect("GPU context required for blend");

        let gpu_ctx = gpu_processing_ctx.gpu().clone();
        let pipeline = gpu_processing_ctx.get_or_create(GpuBlendPipeline::new)?;

        self.apply_gpu(&gpu_ctx, pipeline, &src_gpu, &dst_gpu, &mut output_gpu)
    }
}
