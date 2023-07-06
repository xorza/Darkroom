#[derive(Debug, Hash, PartialEq, Eq, Copy, Clone, Default)]
#[repr(u32)]
pub enum ChannelCount {
    Gray = 1,
    GrayAlpha = 2,
    Rgb = 3,
    #[default]
    Rgba = 4,
}

#[derive(Debug, Hash, PartialEq, Eq, Copy, Clone, Default)]
#[repr(u32)]
pub enum ChannelSize {
    #[default]
    _8bit = 1,
    _16bit = 2,
    _32bit = 4,
    _64bit = 8,
}

#[derive(Debug, Hash, PartialEq, Eq, Copy, Clone, Default)]
#[repr(u32)]
pub enum ChannelType {
    #[default]
    UInt,
    Float,
    Int,
}

#[derive(Clone, Hash, Default, PartialEq, Eq)]
pub struct ColorFormat {
    pub channel_count: ChannelCount,
    pub channel_size: ChannelSize,
    pub channel_type: ChannelType,
}

impl ChannelSize {
    pub fn byte_count(&self) -> u32 {
        *self as u32
    }
    pub(crate) fn from_bit_count(bit_count: u32) -> ChannelSize {
        match bit_count {
            // @formatter:off
            8  => ChannelSize::_8bit ,
            16 => ChannelSize::_16bit,
            32 => ChannelSize::_32bit,
            62 => ChannelSize::_64bit,
            _  => panic!("Invalid channel size: {:?}", bit_count),
            // @formatter:on
        }
    }
}

impl ChannelCount {
    pub fn channel_count(&self) -> u32 {
        *self as u32
    }
    pub fn byte_count(&self, channel_size: ChannelSize) -> u32 {
        self.channel_count() * channel_size.byte_count()
    }
}

impl ColorFormat {
    pub fn byte_count(&self) -> u32 {
        self.channel_count.byte_count(self.channel_size)
    }
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.channel_type == ChannelType::Float {
            match self.channel_size {
                ChannelSize::_8bit | ChannelSize::_16bit =>
                    return Err(anyhow::anyhow!("Unsupported channel size for float: {:?}", self.channel_size)),
                _ => {},
            }
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

impl ColorFormat {
    // gray
    pub const GRAY_U8: ColorFormat = ColorFormat {
        channel_count: ChannelCount::Gray,
        channel_size: ChannelSize::_8bit,
        channel_type: ChannelType::UInt,
    };
    pub const GRAY_U16: ColorFormat = ColorFormat {
        channel_count: ChannelCount::Gray,
        channel_size: ChannelSize::_16bit,
        channel_type: ChannelType::UInt,
    };
    pub const GRAY_U32: ColorFormat = ColorFormat {
        channel_count: ChannelCount::Gray,
        channel_size: ChannelSize::_32bit,
        channel_type: ChannelType::UInt,
    };
    pub const GRAY_U64: ColorFormat = ColorFormat {
        channel_count: ChannelCount::Gray,
        channel_size: ChannelSize::_64bit,
        channel_type: ChannelType::UInt,
    };
    pub const GRAY_I8: ColorFormat = ColorFormat {
        channel_count: ChannelCount::Gray,
        channel_size: ChannelSize::_8bit,
        channel_type: ChannelType::Int,
    };
    pub const GRAY_I16: ColorFormat = ColorFormat {
        channel_count: ChannelCount::Gray,
        channel_size: ChannelSize::_16bit,
        channel_type: ChannelType::Int,
    };
    pub const GRAY_I32: ColorFormat = ColorFormat {
        channel_count: ChannelCount::Gray,
        channel_size: ChannelSize::_32bit,
        channel_type: ChannelType::Int,
    };
    pub const GRAY_I64: ColorFormat = ColorFormat {
        channel_count: ChannelCount::Gray,
        channel_size: ChannelSize::_64bit,
        channel_type: ChannelType::Int,
    };
    pub const GRAY_F32: ColorFormat = ColorFormat {
        channel_count: ChannelCount::Gray,
        channel_size: ChannelSize::_32bit,
        channel_type: ChannelType::Float,
    };
    pub const GRAY_F64: ColorFormat = ColorFormat {
        channel_count: ChannelCount::Gray,
        channel_size: ChannelSize::_64bit,
        channel_type: ChannelType::Float,
    };
    
    //gray alpha
    pub const GRAY_ALPHA_U8: ColorFormat = ColorFormat {
        channel_count: ChannelCount::GrayAlpha,
        channel_size: ChannelSize::_8bit,
        channel_type: ChannelType::UInt,
    };
    pub const GRAY_ALPHA_U16: ColorFormat = ColorFormat {
        channel_count: ChannelCount::GrayAlpha,
        channel_size: ChannelSize::_16bit,
        channel_type: ChannelType::UInt,
    };
    pub const GRAY_ALPHA_U32: ColorFormat = ColorFormat {
        channel_count: ChannelCount::GrayAlpha,
        channel_size: ChannelSize::_32bit,
        channel_type: ChannelType::UInt,
    };
    pub const GRAY_ALPHA_U64: ColorFormat = ColorFormat {
        channel_count: ChannelCount::GrayAlpha,
        channel_size: ChannelSize::_64bit,
        channel_type: ChannelType::UInt,
    };
    pub const GRAY_ALPHA_I8: ColorFormat = ColorFormat {
        channel_count: ChannelCount::GrayAlpha,
        channel_size: ChannelSize::_8bit,
        channel_type: ChannelType::Int,
    };
    pub const GRAY_ALPHA_I16: ColorFormat = ColorFormat {
        channel_count: ChannelCount::GrayAlpha,
        channel_size: ChannelSize::_16bit,
        channel_type: ChannelType::Int,
    };
    pub const GRAY_ALPHA_I32: ColorFormat = ColorFormat {
        channel_count: ChannelCount::GrayAlpha,
        channel_size: ChannelSize::_32bit,
        channel_type: ChannelType::Int,
    };
    pub const GRAY_ALPHA_I64: ColorFormat = ColorFormat {
        channel_count: ChannelCount::GrayAlpha,
        channel_size: ChannelSize::_64bit,
        channel_type: ChannelType::Int,
    };
    pub const GRAY_ALPHA_F32: ColorFormat = ColorFormat {
        channel_count: ChannelCount::GrayAlpha,
        channel_size: ChannelSize::_32bit,
        channel_type: ChannelType::Float,
    };
    pub const GRAY_ALPHA_F64: ColorFormat = ColorFormat {
        channel_count: ChannelCount::GrayAlpha,
        channel_size: ChannelSize::_64bit,
        channel_type: ChannelType::Float,
    };
    
    // rgb
    pub const RGB_U8: ColorFormat = ColorFormat {
        channel_count: ChannelCount::Rgb,
        channel_size: ChannelSize::_8bit,
        channel_type: ChannelType::UInt,
    };
    pub const RGB_U16: ColorFormat = ColorFormat {
        channel_count: ChannelCount::Rgb,
        channel_size: ChannelSize::_16bit,
        channel_type: ChannelType::UInt,
    };
    pub const RGB_U32: ColorFormat = ColorFormat {
        channel_count: ChannelCount::Rgb,
        channel_size: ChannelSize::_32bit,
        channel_type: ChannelType::UInt,
    };
    pub const RGB_U64: ColorFormat = ColorFormat {
        channel_count: ChannelCount::Rgb,
        channel_size: ChannelSize::_64bit,
        channel_type: ChannelType::UInt,
    };
    pub const RGB_I8: ColorFormat = ColorFormat {
        channel_count: ChannelCount::Rgb,
        channel_size: ChannelSize::_8bit,
        channel_type: ChannelType::Int,
    };
    pub const RGB_I16: ColorFormat = ColorFormat {
        channel_count: ChannelCount::Rgb,
        channel_size: ChannelSize::_16bit,
        channel_type: ChannelType::Int,
    };
    pub const RGB_I32: ColorFormat = ColorFormat {
        channel_count: ChannelCount::Rgb,
        channel_size: ChannelSize::_32bit,
        channel_type: ChannelType::Int,
    };
    pub const RGB_I64: ColorFormat = ColorFormat {
        channel_count: ChannelCount::Rgb,
        channel_size: ChannelSize::_64bit,
        channel_type: ChannelType::Int,
    };
    pub const RGB_F32: ColorFormat = ColorFormat {
        channel_count: ChannelCount::Rgb,
        channel_size: ChannelSize::_32bit,
        channel_type: ChannelType::Float,
    };
    pub const RGB_F64: ColorFormat = ColorFormat {
        channel_count: ChannelCount::Rgb,
        channel_size: ChannelSize::_64bit,
        channel_type: ChannelType::Float,
    };

    // rgba
    pub const RGBA_U8: ColorFormat = ColorFormat {
        channel_count: ChannelCount::Rgba,
        channel_size: ChannelSize::_8bit,
        channel_type: ChannelType::UInt,
    };
    pub const RGBA_U16: ColorFormat = ColorFormat {
        channel_count: ChannelCount::Rgba,
        channel_size: ChannelSize::_16bit,
        channel_type: ChannelType::UInt,
    };
    pub const RGBA_U32: ColorFormat = ColorFormat {
        channel_count: ChannelCount::Rgba,
        channel_size: ChannelSize::_32bit,
        channel_type: ChannelType::UInt,
    };
    pub const RGBA_U64: ColorFormat = ColorFormat {
        channel_count: ChannelCount::Rgba,
        channel_size: ChannelSize::_64bit,
        channel_type: ChannelType::UInt,
    };
    pub const RGBA_I8: ColorFormat = ColorFormat {
        channel_count: ChannelCount::Rgba,
        channel_size: ChannelSize::_8bit,
        channel_type: ChannelType::Int,
    };
    pub const RGBA_I16: ColorFormat = ColorFormat {
        channel_count: ChannelCount::Rgba,
        channel_size: ChannelSize::_16bit,
        channel_type: ChannelType::Int,
    };
    pub const RGBA_I32: ColorFormat = ColorFormat {
        channel_count: ChannelCount::Rgba,
        channel_size: ChannelSize::_32bit,
        channel_type: ChannelType::Int,
    };
    pub const RGBA_I64: ColorFormat = ColorFormat {
        channel_count: ChannelCount::Rgba,
        channel_size: ChannelSize::_64bit,
        channel_type: ChannelType::Int,
    };
    pub const RGBA_F32: ColorFormat = ColorFormat {
        channel_count: ChannelCount::Rgba,
        channel_size: ChannelSize::_32bit,
        channel_type: ChannelType::Float,
    };
    pub const RGBA_F64: ColorFormat = ColorFormat {
        channel_count: ChannelCount::Rgba,
        channel_size: ChannelSize::_64bit,
        channel_type: ChannelType::Float,
    };
}