use std::{fmt, io};

use common::Size2us;
use thiserror::Error;

use crate::codec::raster_format::RasterFormat;
use crate::color::color_format::{ColorFormat, SampleType};
use crate::interleaved::interleaved_desc::InterleavedDesc;

/// Why a size and a pixel layout describe no image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum DescError {
    #[error("width must be positive")]
    ZeroWidth,
    #[error("height must be positive")]
    ZeroHeight,
    #[error("only 1 (mono) or 3 (RGB) planes supported, got {count}")]
    Channels { count: usize },
    /// The pixels of `size`, `pixel_byte_len` bytes each, have more bytes than a `usize` counts.
    #[error("{size} pixels of {pixel_byte_len} bytes overflow usize")]
    TooLarge {
        size: Size2us,
        pixel_byte_len: usize,
    },
}

/// Bytes that are not the pixels of the descriptor they came with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("{byte_len} bytes are no {desc} image, which holds {}", desc.byte_len())]
pub struct ByteLenError {
    pub desc: InterleavedDesc,
    pub byte_len: usize,
}

/// Why an interleaved image has no planes of the kind asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum PlanesError {
    /// Planes hold no alpha: [`InterleavedImage::without_alpha`](crate::InterleavedImage::without_alpha)
    /// names the drop.
    #[error("a {color_format} image has an alpha channel, which planes do not hold")]
    Alpha { color_format: ColorFormat },
    /// The image is not `plane_count` channels of `sample_type`.
    #[error("cannot deinterleave a {color_format} image into {plane_count} {sample_type} planes")]
    Mismatch {
        color_format: ColorFormat,
        plane_count: usize,
        sample_type: SampleType,
    },
}

/// Why bytes do not decode as a raster, or an image does not encode as one.
#[derive(Debug, Error)]
pub enum CodecError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("PNG or JPEG codec error: {0}")]
    Image(#[from] image::ImageError),
    #[error("TIFF codec error: {0}")]
    Tiff(#[from] tiff::TiffError),
    /// A PNG or JPEG whose color type no format holds.
    #[error("unsupported color type {0:?}")]
    ImageColor(image::ColorType),
    /// A TIFF whose color type no format holds.
    #[error("unsupported TIFF color type {0:?}")]
    TiffColor(tiff::ColorType),
    /// A TIFF whose sample type no format holds.
    #[error("unsupported TIFF sample type {0}")]
    TiffSamples(TiffSampleType),
    /// The file states a size no image has.
    #[error("the file states no image: {0}")]
    Desc(#[from] DescError),
    /// The file format has no way to store the image's color format.
    #[error("{format} cannot store {color_format}")]
    Unstorable {
        format: RasterFormat,
        color_format: ColorFormat,
    },
    /// An image with a side past the `u32` an encoder takes.
    #[error("{desc} is larger than an image file can hold")]
    TooLarge { desc: InterleavedDesc },
}

/// The TIFF sample types no color format holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TiffSampleType {
    U32,
    U64,
    I8,
    I16,
    I32,
    I64,
    F16,
    F64,
}

impl fmt::Display for TiffSampleType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::U32 => "u32",
            Self::U64 => "u64",
            Self::I8 => "i8",
            Self::I16 => "i16",
            Self::I32 => "i32",
            Self::I64 => "i64",
            Self::F16 => "f16",
            Self::F64 => "f64",
        })
    }
}

/// Why the GPU or one of its buffers is not there.
#[cfg(feature = "wgpu")]
#[derive(Debug, Error)]
pub enum GpuError {
    #[error("no suitable GPU adapter: {0}")]
    Adapter(#[from] wgpu::RequestAdapterError),
    #[error("the GPU device does not open: {0}")]
    Device(#[from] wgpu::RequestDeviceError),
    /// An image whose buffer, `byte_len` bytes, is past the `max` the device binds.
    #[error("a {desc} image needs a {byte_len}-byte buffer; this device binds at most {max}")]
    BufferTooLarge {
        desc: InterleavedDesc,
        byte_len: u64,
        max: u64,
    },
    /// A buffer the device did not map for reading.
    #[error("the GPU buffer does not map: {0}")]
    Map(#[from] wgpu::BufferAsyncError),
}

#[cfg(test)]
mod tests;
