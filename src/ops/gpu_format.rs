use crate::common::color_format::{ChannelCount, ChannelSize, ChannelType, ColorFormat};
use crate::common::error::{Error, Result};

// Format type constants matching GPU shader definitions.
pub(crate) const FORMAT_L_U8: u32 = 0;
pub(crate) const FORMAT_LA_U8: u32 = 1;
pub(crate) const FORMAT_RGB_U8: u32 = 2;
pub(crate) const FORMAT_RGBA_U8: u32 = 3;
pub(crate) const FORMAT_L_F32: u32 = 4;
pub(crate) const FORMAT_LA_F32: u32 = 5;
pub(crate) const FORMAT_RGB_F32: u32 = 6;
pub(crate) const FORMAT_RGBA_F32: u32 = 7;
pub(crate) const FORMAT_L_U16: u32 = 8;
pub(crate) const FORMAT_LA_U16: u32 = 9;
pub(crate) const FORMAT_RGB_U16: u32 = 10;
pub(crate) const FORMAT_RGBA_U16: u32 = 11;

/// Number of 256-wide workgroups for a per-pixel compute shader, accounting for the
/// narrow-format packing the shaders use (L_U8 packs 4 pixels/item, LA_U8 & L_U16 pack 2).
pub(crate) fn workgroup_count(format_type: u32, width: usize, height: usize) -> u32 {
    let work_items = match format_type {
        FORMAT_L_U8 => width.div_ceil(4) * height,
        FORMAT_LA_U8 | FORMAT_L_U16 => width.div_ceil(2) * height,
        _ => width * height,
    };
    work_items.div_ceil(256) as u32
}

pub(crate) fn get_format_type(format: ColorFormat) -> Result<u32> {
    match (
        format.channel_count,
        format.channel_size,
        format.channel_type,
    ) {
        (ChannelCount::L, ChannelSize::_8bit, ChannelType::UInt) => Ok(FORMAT_L_U8),
        (ChannelCount::LA, ChannelSize::_8bit, ChannelType::UInt) => Ok(FORMAT_LA_U8),
        (ChannelCount::Rgb, ChannelSize::_8bit, ChannelType::UInt) => Ok(FORMAT_RGB_U8),
        (ChannelCount::Rgba, ChannelSize::_8bit, ChannelType::UInt) => Ok(FORMAT_RGBA_U8),
        (ChannelCount::L, ChannelSize::_32bit, ChannelType::Float) => Ok(FORMAT_L_F32),
        (ChannelCount::LA, ChannelSize::_32bit, ChannelType::Float) => Ok(FORMAT_LA_F32),
        (ChannelCount::Rgb, ChannelSize::_32bit, ChannelType::Float) => Ok(FORMAT_RGB_F32),
        (ChannelCount::Rgba, ChannelSize::_32bit, ChannelType::Float) => Ok(FORMAT_RGBA_F32),
        (ChannelCount::L, ChannelSize::_16bit, ChannelType::UInt) => Ok(FORMAT_L_U16),
        (ChannelCount::LA, ChannelSize::_16bit, ChannelType::UInt) => Ok(FORMAT_LA_U16),
        (ChannelCount::Rgb, ChannelSize::_16bit, ChannelType::UInt) => Ok(FORMAT_RGB_U16),
        (ChannelCount::Rgba, ChannelSize::_16bit, ChannelType::UInt) => Ok(FORMAT_RGBA_U16),
        _ => Err(Error::UnsupportedFormat(format!(
            "GPU does not support format: {format}"
        ))),
    }
}
