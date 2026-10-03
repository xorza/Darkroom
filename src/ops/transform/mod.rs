mod cpu;
#[cfg(feature = "wgpu")]
mod gpu;
#[cfg(feature = "wgpu")]
pub(crate) mod pipeline;

use glam::{Affine2, Vec2};

#[cfg(feature = "wgpu")]
use crate::gpu::Gpu;
#[cfg(feature = "wgpu")]
use crate::gpu::gpu_image::GpuImage;
use crate::image::Image;
#[cfg(feature = "wgpu")]
use crate::ops::transform::pipeline::GpuTransformPipeline;

/// Filter mode for image sampling during transformation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FilterMode {
    /// Nearest neighbor sampling - fast but can produce aliasing.
    Nearest,
    /// Bilinear interpolation - smoother results.
    #[default]
    Bilinear,
}

/// Image transformation parameters.
#[derive(Debug, Clone, Copy)]
pub struct Transform {
    /// The affine transformation to apply.
    pub transform: Affine2,
    /// The filter mode for sampling.
    pub filter: FilterMode,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            transform: Affine2::IDENTITY,
            filter: FilterMode::default(),
        }
    }
}

impl Transform {
    /// Creates a new identity transform.
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies a scale transformation.
    #[must_use]
    pub fn scale(mut self, scale: Vec2) -> Self {
        self.transform *= Affine2::from_scale(scale);
        self
    }

    /// Applies a rotation transformation (angle in radians).
    #[must_use]
    pub fn rotate(mut self, angle: f32) -> Self {
        self.transform *= Affine2::from_angle(angle);
        self
    }

    /// Applies a rotation around a center point (angle in radians).
    #[must_use]
    pub fn rotate_around(mut self, angle: f32, center: Vec2) -> Self {
        self.transform *= Affine2::from_translation(center)
            * Affine2::from_angle(angle)
            * Affine2::from_translation(-center);
        self
    }

    /// Applies a translation transformation.
    #[must_use]
    pub fn translate(mut self, translation: Vec2) -> Self {
        self.transform *= Affine2::from_translation(translation);
        self
    }

    /// Sets the filter mode.
    #[must_use]
    pub const fn filter(mut self, filter: FilterMode) -> Self {
        self.filter = filter;
        self
    }

    /// Whether the transform and its inverse both map pixels to pixels: every
    /// coefficient is finite, the determinant is normal, and the inverse is
    /// finite. A transform built from untrusted values is checked with this
    /// before it is applied.
    pub fn is_invertible(&self) -> bool {
        self.transform.is_finite()
            && self.transform.matrix2.determinant().is_normal()
            && self.transform.inverse().is_finite()
    }

    /// The output-to-input map every backend samples through.
    ///
    /// # Panics
    /// Unless [`Self::is_invertible`].
    pub(crate) fn inverse(&self) -> Affine2 {
        assert!(
            self.is_invertible(),
            "the transform is not invertible: {:?}",
            self.transform
        );
        self.transform.inverse()
    }

    /// Applies the affine transform on the CPU, sampling `input` into `output`.
    ///
    /// `output`'s descriptor sets the result dimensions (which may differ from
    /// the input's); output pixels whose source maps outside the input are zero.
    /// Every channel, alpha included, interpolates the same way.
    ///
    /// # Panics
    /// Panics if input and output have different color formats, or if the
    /// transform is not invertible.
    pub fn apply_cpu(&self, input: &Image, output: &mut Image) {
        cpu::apply(self, input, output);
    }

    /// Applies the transform on the GPU, sampling `input` into `output`, as
    /// [`Self::apply_cpu`] does.
    ///
    /// # Panics
    /// Panics if input and output have different color formats, or if the
    /// transform is not invertible.
    #[cfg(feature = "wgpu")]
    pub fn apply_gpu(
        &self,
        ctx: &Gpu,
        pipeline: &GpuTransformPipeline,
        input: &GpuImage,
        output: &mut GpuImage,
    ) {
        gpu::apply(self, ctx, pipeline, input, output);
    }
}

#[cfg(feature = "bench")]
pub(crate) mod bench;
