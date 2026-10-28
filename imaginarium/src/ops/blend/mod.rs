mod cpu;
#[cfg(feature = "wgpu")]
mod gpu;
#[cfg(feature = "wgpu")]
mod pipeline;

use strum::{EnumIter, EnumString, VariantNames};

#[cfg(feature = "wgpu")]
use crate::gpu::Gpu;
#[cfg(feature = "wgpu")]
use crate::gpu::gpu_image::GpuImage;
use crate::interleaved::interleaved_image::InterleavedImage;
use crate::ops::clamp::Clamp;

/// How a source channel is combined with the destination channel under it.
///
/// The formulas are stated on the variants because the names do not fix them:
/// `Subtract` takes `src` *from* `dst`, and `Overlay` switches formula on the
/// destination rather than the source. `BlendMode::blend` is the one
/// definition every backend — scalar, SIMD and the GPU shader — evaluates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, EnumIter, EnumString, VariantNames)]
#[repr(u8)]
pub enum BlendMode {
    /// `src` — the destination shows through only via the alpha mix.
    #[default]
    Normal,
    /// `src + dst`.
    Add,
    /// `dst - src`.
    Subtract,
    /// `src * dst`.
    Multiply,
    /// `1 - (1 - src) * (1 - dst)` — Multiply on the inverted operands.
    Screen,
    /// Multiply where `dst < 0.5`, Screen above it.
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
    /// The ends of the range each color channel's result is held to, after the alpha mix; an
    /// integer one saturates to its range whatever this is, and an alpha channel is not held.
    pub clamp: Clamp,
}

impl Default for Blend {
    fn default() -> Self {
        Self::new(BlendMode::Normal, 1.0)
    }
}

impl Blend {
    /// `mode` at `alpha` under [`Clamp::DEFAULT`].
    pub const fn new(mode: BlendMode, alpha: f32) -> Self {
        Self {
            mode,
            alpha,
            clamp: Clamp::DEFAULT,
        }
    }

    /// Builder method to set blend mode.
    #[must_use]
    pub const fn mode(mut self, mode: BlendMode) -> Self {
        self.mode = mode;
        self
    }

    /// Builder method to set alpha.
    #[must_use]
    pub const fn alpha(mut self, alpha: f32) -> Self {
        self.alpha = alpha;
        self
    }

    /// Builder method to set the clamp.
    #[must_use]
    pub const fn clamp(mut self, clamp: Clamp) -> Self {
        self.clamp = clamp;
        self
    }

    /// Blends `src` (the top layer) over `dst` (the bottom one) into `output`,
    /// on the CPU. An alpha channel, where the format has one, is always mixed
    /// by `alpha` alone — the blend mode applies to the color channels.
    ///
    /// # Panics
    /// Panics unless all three images share a descriptor.
    pub fn apply_cpu(
        &self,
        src: &InterleavedImage,
        dst: &InterleavedImage,
        output: &mut InterleavedImage,
    ) {
        cpu::apply(*self, src, dst, output);
    }

    /// Blends `src` over `dst` into `output` on the GPU, as [`Self::apply_cpu`]
    /// does.
    ///
    /// # Panics
    /// Panics unless all three images share a descriptor.
    #[cfg(feature = "wgpu")]
    pub fn apply_gpu(&self, gpu: &Gpu, src: &GpuImage, dst: &GpuImage, output: &mut GpuImage) {
        gpu::apply(*self, gpu, src, dst, output);
    }
}

#[cfg(feature = "internals")]
pub(crate) mod bench;

#[cfg(feature = "internals")]
pub(crate) mod internals {
    use crate::ops::blend::BlendMode;

    impl BlendMode {
        /// Combines two normalized `[0, 1]` channel values, then mixes the result
        /// back over `dst` by `alpha`: `blended * alpha + dst * (1 - alpha)`.
        ///
        /// The scalar reference, which the vector kernels hold to the bit. The result is left
        /// unclamped: [`Blend::clamp`](crate::ops::blend::Blend::clamp) holds it after, and
        /// each storage type saturates to its own range on write.
        #[inline]
        pub(crate) fn blend(self, src: f32, dst: f32, alpha: f32) -> f32 {
            let blended = match self {
                Self::Normal => src,
                Self::Add => src + dst,
                Self::Subtract => dst - src,
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
}
