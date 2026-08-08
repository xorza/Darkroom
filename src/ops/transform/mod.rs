#[cfg(feature = "bench")]
pub(crate) mod bench;
mod cpu;
#[cfg(feature = "wgpu")]
mod gpu;
#[cfg(feature = "wgpu")]
pub(crate) mod pipeline;

use glam::{Affine2, Vec2};

use crate::image::Image;

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

    /// Sets the affine transformation directly.
    pub fn affine(mut self, transform: Affine2) -> Self {
        self.transform = transform;
        self
    }

    /// Applies a scale transformation.
    pub fn scale(mut self, scale: Vec2) -> Self {
        self.transform *= Affine2::from_scale(scale);
        self
    }

    /// Applies a rotation transformation (angle in radians).
    pub fn rotate(mut self, angle: f32) -> Self {
        self.transform *= Affine2::from_angle(angle);
        self
    }

    /// Applies a rotation around a center point (angle in radians).
    pub fn rotate_around(mut self, angle: f32, center: Vec2) -> Self {
        self.transform *= Affine2::from_translation(center)
            * Affine2::from_angle(angle)
            * Affine2::from_translation(-center);
        self
    }

    /// Applies a translation transformation.
    pub fn translate(mut self, translation: Vec2) -> Self {
        self.transform *= Affine2::from_translation(translation);
        self
    }

    /// Sets the filter mode.
    pub fn filter(mut self, filter: FilterMode) -> Self {
        self.filter = filter;
        self
    }

    /// Applies the affine transform on the CPU, sampling `input` into `output`.
    ///
    /// `output`'s descriptor sets the result dimensions (which may differ from
    /// the input's); output pixels whose source maps outside the input are zero.
    /// Grayscale and RGB sample against an implicit opaque alpha; RGBA carries
    /// its own alpha through the same interpolation as the color channels.
    ///
    /// # Panics
    /// Panics if input and output have different color formats.
    pub fn apply_cpu(&self, input: &Image, output: &mut Image) {
        cpu::apply(self, input, output);
    }
}
