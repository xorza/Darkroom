#[cfg(feature = "internals")]
pub mod bench;
mod color;
mod error;
#[cfg(feature = "wgpu")]
mod gpu;
mod image;
#[cfg(test)]
mod internals;
mod ops;
mod planar;

pub use crate::color::color_format::{ChannelCount, ColorFormat, SampleType};
pub use crate::error::{ByteLenError, CodecError, DescError, PlanesError, TiffSampleType};

pub use crate::image::InterleavedImage;
pub use crate::image::image_desc::InterleavedDesc;
pub use crate::image::raster_format::{RasterFormat, SUPPORTED_EXTENSIONS};

pub use crate::planar::channel_layout::ChannelLayout;
pub use crate::planar::planar_desc::PlanarDesc;
pub use crate::planar::planar_image::PlanarImage;

pub use crate::ops::affine_warp::{AffineWarp, FilterMode};
pub use crate::ops::blend::{Blend, BlendMode};
pub use crate::ops::contrast_brightness::ContrastBrightness;
pub use crate::ops::preview::Preview;

#[cfg(feature = "wgpu")]
pub use crate::{
    error::GpuError,
    gpu::Gpu,
    gpu::gpu_ctx::{GpuCtx, GpuPipeline},
    gpu::gpu_image::GpuImage,
    ops::affine_warp::pipeline::GpuAffineWarpPipeline,
    ops::blend::pipeline::GpuBlendPipeline,
    ops::contrast_brightness::pipeline::GpuContrastBrightnessPipeline,
};
