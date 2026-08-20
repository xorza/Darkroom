mod cpu;
#[cfg(feature = "wgpu")]
mod gpu;
#[cfg(feature = "wgpu")]
pub(crate) mod pipeline;

use strum_macros::{EnumIter, EnumString, VariantNames};

#[cfg(feature = "wgpu")]
use crate::common::error::Result;
#[cfg(feature = "wgpu")]
use crate::gpu::Gpu;
#[cfg(feature = "wgpu")]
use crate::gpu::gpu_image::GpuImage;
use crate::image::Image;
#[cfg(feature = "wgpu")]
use crate::ops::blend::pipeline::GpuBlendPipeline;

/// How a source channel is combined with the destination channel under it.
///
/// The formulas are stated on the variants because the names do not fix them:
/// `Subtract` takes `src` *from* `dst`, and `Overlay` switches formula on the
/// destination rather than the source. [`BlendMode::blend`] is the one
/// definition every backend — scalar, SIMD and the GPU shader — evaluates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, EnumIter, EnumString, VariantNames)]
#[repr(u8)]
pub enum BlendMode {
    /// `src` — the destination shows through only via the alpha mix.
    #[default]
    Normal,
    /// `src + dst`, clamped at one.
    Add,
    /// `dst - src`, clamped at zero.
    Subtract,
    /// `src * dst`.
    Multiply,
    /// `1 - (1 - src) * (1 - dst)` — Multiply on the inverted operands.
    Screen,
    /// Multiply where `dst < 0.5`, Screen above it.
    Overlay,
}

impl BlendMode {
    /// Combines two normalized `[0, 1]` channel values, then mixes the result
    /// back over `dst` by `alpha`: `blended * alpha + dst * (1 - alpha)`.
    ///
    /// Every CPU backend routes through this: the scalar path per channel, the
    /// SIMD kernels for their sub-vector tails, so a tail can never disagree
    /// with the vector body it follows. The result is left unclamped — each
    /// storage type clamps to its own range on write.
    #[inline]
    fn blend(self, src: f32, dst: f32, alpha: f32) -> f32 {
        let blended = match self {
            Self::Normal => src,
            Self::Add => (src + dst).min(1.0),
            Self::Subtract => (dst - src).max(0.0),
            Self::Multiply => src * dst,
            Self::Screen => 1.0 - (1.0 - src) * (1.0 - dst),
            Self::Overlay => {
                if dst < 0.5 {
                    2.0 * src * dst
                } else {
                    1.0 - 2.0 * (1.0 - src) * (1.0 - dst)
                }
            }
        };
        blended * alpha + dst * (1.0 - alpha)
    }
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

    /// Blends `src` (the top layer) over `dst` (the bottom one) into `output`,
    /// on the CPU. An alpha channel, where the format has one, is always mixed
    /// by `alpha` alone — the blend mode applies to the color channels.
    ///
    /// # Panics
    /// Panics unless all three images share a descriptor.
    pub fn apply_cpu(&self, src: &Image, dst: &Image, output: &mut Image) {
        cpu::apply(self, src, dst, output);
    }

    /// Blends `src` over `dst` into `output` on the GPU, for U8 and F32 storage
    /// in L, LA, RGB and RGBA.
    ///
    /// # Panics
    /// Panics unless all three images share dimensions and color format.
    #[cfg(feature = "wgpu")]
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
}
