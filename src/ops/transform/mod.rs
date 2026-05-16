mod gpu;
pub(crate) mod pipeline;

use glam::{Affine2, Vec2};

use crate::common::color_format::ColorFormat;
use crate::common::error::Result;
use crate::ops::backend_selection::{Backend, select_backend};
use crate::processing_context::ProcessingContext;
use crate::processing_context::image_buffer::ImageBuffer;

const SUPPORTED_GPU_FORMATS: &[ColorFormat] = &[
    ColorFormat::L_U8,
    ColorFormat::LA_U8,
    ColorFormat::RGB_U8,
    ColorFormat::RGBA_U8,
    ColorFormat::L_U16,
    ColorFormat::LA_U16,
    ColorFormat::RGB_U16,
    ColorFormat::RGBA_U16,
    ColorFormat::L_F32,
    ColorFormat::LA_F32,
    ColorFormat::RGB_F32,
    ColorFormat::RGBA_F32,
];

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

    /// Applies the operation, automatically choosing CPU or GPU based on data location.
    ///
    /// Transform is GPU-only, so this always uses GPU.
    ///
    /// # Errors
    /// Returns an error if:
    /// - Input and output have different color formats
    /// - The color format is not supported (only RGBA_U8 is supported)
    pub fn execute(
        &self,
        ctx: &mut ProcessingContext,
        input: &ImageBuffer,
        output: &mut ImageBuffer,
    ) -> Result<()> {
        let backend = select_backend(
            ctx,
            &[input, output],
            &[], // No CPU support
            SUPPORTED_GPU_FORMATS,
            "Transform",
        )?;

        match backend {
            Backend::Gpu => self.execute_gpu(ctx, input, output),
            Backend::Cpu => unreachable!("Transform does not support CPU"),
        }
    }
}
