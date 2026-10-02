#[cfg(feature = "bench")]
pub mod bench;
mod common;
pub mod cpu_features;
pub mod drawing;
#[cfg(feature = "wgpu")]
mod gpu;
mod image;
mod ops;

pub use glam::{Affine2, Vec2};

pub use crate::common::color::Color;
pub use crate::common::color_format::{ALL_FORMATS, ChannelCount, ColorFormat, SampleType};
pub use crate::common::error::{Error, Result};

pub use crate::common::buffer2::Buffer2;
pub use crate::image::image_desc::ImageDesc;
pub use crate::image::{Image, SUPPORTED_EXTENSIONS};

pub use crate::ops::blend::{Blend, BlendMode};
pub use crate::ops::contrast_brightness::ContrastBrightness;
pub use crate::ops::preview::Preview;
pub use crate::ops::transform::{FilterMode, Transform};

#[cfg(feature = "wgpu")]
pub use crate::{
    gpu::Gpu,
    gpu::context::{GpuContext, GpuPipeline},
    gpu::gpu_image::GpuImage,
    ops::blend::pipeline::GpuBlendPipeline,
    ops::contrast_brightness::pipeline::GpuContrastBrightnessPipeline,
    ops::transform::pipeline::GpuTransformPipeline,
};
