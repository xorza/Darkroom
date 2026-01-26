use crate::common::error::{Error, Result};

#[derive(Debug, Hash, PartialEq, Eq, Copy, Clone, Default)]
#[repr(u8)]
pub enum ChannelCount {
    L = 1,
    LA = 2,
    Rgb = 3,
    #[default]
    Rgba = 4,
}

#[derive(Debug, Hash, PartialEq, Eq, Copy, Clone, Default)]
#[repr(u8)]
pub enum ChannelSize {
    #[default]
    _8bit = 1,
    _16bit = 2,
    _32bit = 4,
}

#[derive(Debug, Hash, PartialEq, Eq, Copy, Clone, Default)]
#[repr(u8)]
pub enum ChannelType {
    #[default]
    UInt,
    Float,
}

#[derive(Clone, Copy, Debug, Hash, Default, PartialEq, Eq)]
pub struct ColorFormat {
    pub channel_count: ChannelCount,
    pub channel_size: ChannelSize,
    pub channel_type: ChannelType,
}

impl ChannelCount {
    pub fn channel_count(&self) -> u8 {
        *self as u8
    }
    pub fn byte_count(&self, channel_size: ChannelSize) -> u8 {
        self.channel_count() * channel_size.byte_count()
    }
}

impl ChannelSize {
    pub fn byte_count(&self) -> u8 {
        *self as u8
    }
    pub(crate) fn from_bit_count(bit_count: u8) -> Result<ChannelSize> {
        match bit_count {
            8 => Ok(ChannelSize::_8bit),
            16 => Ok(ChannelSize::_16bit),
            32 => Ok(ChannelSize::_32bit),
            _ => Err(Error::InvalidColorFormat(format!(
                "invalid channel size: {} bits",
                bit_count
            ))),
        }
    }
}

impl ColorFormat {
    pub fn byte_count(&self) -> u8 {
        self.channel_count.byte_count(self.channel_size)
    }

    pub fn is_supported(&self) -> bool {
        ALL_FORMATS.contains(self)
    }

    pub fn validate(&self) -> Result<()> {
        if !self.is_supported() {
            return Err(Error::InvalidColorFormat(format!(
                "unsupported color format: {:?}",
                self
            )));
        }
        Ok(())
    }
}

impl From<(ChannelCount, ChannelSize, ChannelType)> for ColorFormat {
    fn from(value: (ChannelCount, ChannelSize, ChannelType)) -> Self {
        ColorFormat {
            channel_count: value.0,
            channel_size: value.1,
            channel_type: value.2,
        }
    }
}

macro_rules! define_color_formats {
    ($(($prefix:ident, $count:ident)),+ $(,)?) => {
        paste::paste! {
            impl ColorFormat {
                $(
                    pub const [<$prefix _U8>]:  ColorFormat = ColorFormat { channel_count: ChannelCount::$count, channel_size: ChannelSize::_8bit,  channel_type: ChannelType::UInt };
                    pub const [<$prefix _U16>]: ColorFormat = ColorFormat { channel_count: ChannelCount::$count, channel_size: ChannelSize::_16bit, channel_type: ChannelType::UInt };
                    pub const [<$prefix _F32>]: ColorFormat = ColorFormat { channel_count: ChannelCount::$count, channel_size: ChannelSize::_32bit, channel_type: ChannelType::Float };
                )+
            }
        }
    };
}

define_color_formats!((L, L), (LA, LA), (RGB, Rgb), (RGBA, Rgba),);

impl std::fmt::Display for ChannelCount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChannelCount::L => write!(f, "L"),
            ChannelCount::LA => write!(f, "LA"),
            ChannelCount::Rgb => write!(f, "RGB"),
            ChannelCount::Rgba => write!(f, "RGBA"),
        }
    }
}

impl std::fmt::Display for ChannelSize {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChannelSize::_8bit => write!(f, "8"),
            ChannelSize::_16bit => write!(f, "16"),
            ChannelSize::_32bit => write!(f, "32"),
        }
    }
}

impl std::fmt::Display for ChannelType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChannelType::UInt => write!(f, "u"),
            ChannelType::Float => write!(f, "f"),
        }
    }
}

impl std::fmt::Display for ColorFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} {}{}",
            self.channel_count, self.channel_type, self.channel_size
        )
    }
}

/// All supported color formats.
pub const ALL_FORMATS: &[ColorFormat] = &[
    //
    ColorFormat::L_U8,
    ColorFormat::L_U16,
    ColorFormat::L_F32,
    //
    ColorFormat::LA_U8,
    ColorFormat::LA_U16,
    ColorFormat::LA_F32,
    //
    ColorFormat::RGB_U8,
    ColorFormat::RGB_U16,
    ColorFormat::RGB_F32,
    //
    ColorFormat::RGBA_U8,
    ColorFormat::RGBA_U16,
    ColorFormat::RGBA_F32,
];

/// Formats with alpha channel (LA and RGBA).
pub const ALPHA_FORMATS: &[ColorFormat] = &[
    //
    ColorFormat::LA_U8,
    ColorFormat::LA_U16,
    ColorFormat::LA_F32,
    //
    ColorFormat::RGBA_U8,
    ColorFormat::RGBA_U16,
    ColorFormat::RGBA_F32,
];
