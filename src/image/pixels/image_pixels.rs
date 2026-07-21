//! Runtime-format interleaved storage owned by [`Image`](crate::image::Image).

use crate::common::color_format::{ChannelCount, ChannelSize, ChannelType, ColorFormat};
use crate::image::ImageDesc;
use crate::image::pixels::interleaved::InterleavedPixels;

#[derive(Debug, Clone)]
#[allow(non_camel_case_types)] // Variants mirror the corresponding `ColorFormat` names.
pub(crate) enum ImagePixels {
    L_U8(InterleavedPixels<1, u8>),
    L_U16(InterleavedPixels<1, u16>),
    L_F32(InterleavedPixels<1, f32>),
    RGB_U8(InterleavedPixels<3, u8>),
    RGB_U16(InterleavedPixels<3, u16>),
    RGB_F32(InterleavedPixels<3, f32>),
    RGBA_U8(InterleavedPixels<4, u8>),
    RGBA_U16(InterleavedPixels<4, u16>),
    RGBA_F32(InterleavedPixels<4, f32>),
}

macro_rules! with_pixels {
    ($self:expr, $pixels:ident => $body:expr) => {
        match $self {
            ImagePixels::L_U8($pixels) => $body,
            ImagePixels::L_U16($pixels) => $body,
            ImagePixels::L_F32($pixels) => $body,
            ImagePixels::RGB_U8($pixels) => $body,
            ImagePixels::RGB_U16($pixels) => $body,
            ImagePixels::RGB_F32($pixels) => $body,
            ImagePixels::RGBA_U8($pixels) => $body,
            ImagePixels::RGBA_U16($pixels) => $body,
            ImagePixels::RGBA_F32($pixels) => $body,
        }
    };
}

macro_rules! by_format {
    ($format:expr, $constructor:ident ( $($arg:expr),* )) => {{
        let format = $format;
        match (
            format.channel_count,
            format.channel_size,
            format.channel_type,
        ) {
            (ChannelCount::L, ChannelSize::_8bit, ChannelType::UInt) => Self::L_U8(InterleavedPixels::$constructor($($arg),*)),
            (ChannelCount::L, ChannelSize::_16bit, ChannelType::UInt) => Self::L_U16(InterleavedPixels::$constructor($($arg),*)),
            (ChannelCount::L, ChannelSize::_32bit, ChannelType::Float) => Self::L_F32(InterleavedPixels::$constructor($($arg),*)),
            (ChannelCount::Rgb, ChannelSize::_8bit, ChannelType::UInt) => Self::RGB_U8(InterleavedPixels::$constructor($($arg),*)),
            (ChannelCount::Rgb, ChannelSize::_16bit, ChannelType::UInt) => Self::RGB_U16(InterleavedPixels::$constructor($($arg),*)),
            (ChannelCount::Rgb, ChannelSize::_32bit, ChannelType::Float) => Self::RGB_F32(InterleavedPixels::$constructor($($arg),*)),
            (ChannelCount::Rgba, ChannelSize::_8bit, ChannelType::UInt) => Self::RGBA_U8(InterleavedPixels::$constructor($($arg),*)),
            (ChannelCount::Rgba, ChannelSize::_16bit, ChannelType::UInt) => Self::RGBA_U16(InterleavedPixels::$constructor($($arg),*)),
            (ChannelCount::Rgba, ChannelSize::_32bit, ChannelType::Float) => Self::RGBA_F32(InterleavedPixels::$constructor($($arg),*)),
            _ => panic!("unsupported image format {format:?}"),
        }
    }};
}

impl ImagePixels {
    pub(crate) fn new_zeroed(format: ColorFormat, width: usize, height: usize) -> Self {
        by_format!(format, new_zeroed(width, height))
    }

    pub(crate) fn from_bytes(
        format: ColorFormat,
        width: usize,
        height: usize,
        bytes: &[u8],
    ) -> Self {
        by_format!(format, from_bytes(width, height, bytes))
    }

    pub(crate) fn desc(&self) -> ImageDesc {
        macro_rules! desc {
            ($pixels:expr, $format:expr) => {
                ImageDesc::new($pixels.buffer.width(), $pixels.buffer.height(), $format)
            };
        }

        match self {
            Self::L_U8(pixels) => desc!(pixels, ColorFormat::L_U8),
            Self::L_U16(pixels) => desc!(pixels, ColorFormat::L_U16),
            Self::L_F32(pixels) => desc!(pixels, ColorFormat::L_F32),
            Self::RGB_U8(pixels) => desc!(pixels, ColorFormat::RGB_U8),
            Self::RGB_U16(pixels) => desc!(pixels, ColorFormat::RGB_U16),
            Self::RGB_F32(pixels) => desc!(pixels, ColorFormat::RGB_F32),
            Self::RGBA_U8(pixels) => desc!(pixels, ColorFormat::RGBA_U8),
            Self::RGBA_U16(pixels) => desc!(pixels, ColorFormat::RGBA_U16),
            Self::RGBA_F32(pixels) => desc!(pixels, ColorFormat::RGBA_F32),
        }
    }

    pub(crate) fn bytes(&self) -> &[u8] {
        with_pixels!(self, pixels => pixels.as_bytes())
    }

    pub(crate) fn bytes_mut(&mut self) -> &mut [u8] {
        with_pixels!(self, pixels => pixels.as_bytes_mut())
    }
}
