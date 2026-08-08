mod cpu;
#[cfg(feature = "wgpu")]
mod gpu;
#[cfg(feature = "wgpu")]
pub(crate) mod pipeline;

use strum_macros::{EnumString, VariantNames};

use crate::image::Image;

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
}
