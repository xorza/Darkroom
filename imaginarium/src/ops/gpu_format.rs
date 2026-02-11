pub use crate::common::color_format::ColorFormat;
use crate::common::color_format::{ChannelCount, ChannelSize, ChannelType};
use crate::common::error::{Error, Result};

// Format type constants matching GPU shader definitions.
pub const FORMAT_L_U8: u32 = 0;
pub const FORMAT_LA_U8: u32 = 1;
pub const FORMAT_RGB_U8: u32 = 2;
pub const FORMAT_RGBA_U8: u32 = 3;
pub const FORMAT_L_F32: u32 = 4;
pub const FORMAT_LA_F32: u32 = 5;
pub const FORMAT_RGB_F32: u32 = 6;
pub const FORMAT_RGBA_F32: u32 = 7;
pub const FORMAT_L_U16: u32 = 8;
pub const FORMAT_LA_U16: u32 = 9;
pub const FORMAT_RGB_U16: u32 = 10;
pub const FORMAT_RGBA_U16: u32 = 11;

pub fn get_format_type(format: ColorFormat) -> Result<u32> {
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
            "GPU does not support format: {}",
            format
        ))),
    }
}
