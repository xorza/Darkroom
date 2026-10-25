#[cfg(feature = "internals")]
pub mod bench;
mod color;
pub mod drawing;
mod error;
#[cfg(feature = "wgpu")]
mod gpu;
mod image;
#[cfg(test)]
mod internals;
mod ops;
mod simd_tier;

pub use glam::{Affine2, Vec2};

pub use crate::color::color_format::{ALL_FORMATS, ChannelCount, ColorFormat, SampleType};
pub use crate::drawing::color::Color;
pub use crate::error::{Error, Result};

pub use crate::image::Image;
pub use crate::image::file_format::{FileFormat, SUPPORTED_EXTENSIONS};
pub use crate::image::image_desc::ImageDesc;

pub use crate::ops::blend::{Blend, BlendMode};
pub use crate::ops::contrast_brightness::ContrastBrightness;
pub use crate::ops::preview::Preview;
pub use crate::ops::transform::{FilterMode, Transform};
pub use crate::simd_tier::SimdTier;

#[cfg(feature = "wgpu")]
pub use crate::{
    gpu::Gpu,
    gpu::context::{GpuContext, GpuPipeline},
    gpu::gpu_image::GpuImage,
    ops::blend::pipeline::GpuBlendPipeline,
    ops::contrast_brightness::pipeline::GpuContrastBrightnessPipeline,
    ops::transform::pipeline::GpuTransformPipeline,
};
