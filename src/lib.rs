/// Shorter alias for `#[cfg(target_arch = "x86_64")]`
#[macro_export]
macro_rules! cfg_x86_64 {
    ($($item:item)*) => {
        $(
            #[cfg(target_arch = "x86_64")]
            $item
        )*
    };
}

/// Shorter alias for `#[cfg(target_arch = "aarch64")]`
#[macro_export]
macro_rules! cfg_aarch64 {
    ($($item:item)*) => {
        $(
            #[cfg(target_arch = "aarch64")]
            $item
        )*
    };
}

mod common;
pub mod cpu_features;
pub mod drawing;
#[cfg(feature = "wgpu")]
mod gpu;
mod image;
mod ops;
mod processing_context;

pub use glam::{Affine2, Vec2};

pub use crate::common::color::Color;
pub use crate::common::color_format::{
    ALL_FORMATS, ALPHA_FORMATS, ChannelCount, ChannelSize, ChannelType, ColorFormat,
};
pub use crate::common::error::{Error, Result};

pub use crate::common::buffer2::Buffer2;
pub use crate::image::image_data::DeinterleavedImageData;
pub use crate::image::{Image, ImageDesc, SUPPORTED_EXTENSIONS};

pub use crate::processing_context::ProcessingContext;
pub use crate::processing_context::image_buffer::ImageBuffer;

pub use crate::ops::blend::{Blend, BlendMode};
pub use crate::ops::contrast_brightness::ContrastBrightness;

#[cfg(feature = "wgpu")]
pub use crate::{
    gpu::Gpu,
    gpu::gpu_image::GpuImage,
    ops::blend::pipeline::GpuBlendPipeline,
    ops::contrast_brightness::pipeline::GpuContrastBrightnessPipeline,
    ops::transform::pipeline::GpuTransformPipeline,
    ops::transform::{FilterMode, Transform},
    processing_context::gpu_context::{GpuContext, GpuPipeline},
};
