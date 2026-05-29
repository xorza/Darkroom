mod cpu;
#[cfg(feature = "wgpu")]
mod gpu;
#[cfg(feature = "wgpu")]
pub(crate) mod pipeline;

use strum_macros::{EnumString, VariantNames};

use crate::common::color_format::ALL_FORMATS;
use crate::common::error::Result;
use crate::image::Image;
use crate::ops::backend_selection::{Backend, select_backend};
use crate::processing_context::ProcessingContext;
use crate::processing_context::image_buffer::ImageBuffer;

/// Blend modes for combining two images.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, EnumString, VariantNames)]
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
        let backend = select_backend(ctx, &[src, dst, output], ALL_FORMATS, ALL_FORMATS, "Blend")?;

        match backend {
            #[cfg(feature = "wgpu")]
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
}
