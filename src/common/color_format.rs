use std::fmt;

/// How many interleaved channels a pixel holds.
#[derive(Debug, Hash, PartialEq, Eq, Copy, Clone)]
#[repr(u8)]
pub enum ChannelCount {
    L = 1,
    Rgb = 3,
    Rgba = 4,
}

/// The storage type of one channel value.
#[derive(Debug, Hash, PartialEq, Eq, Copy, Clone)]
pub enum SampleType {
    U8,
    U16,
    F32,
}

/// A pixel format: channel count × sample type. Every one of the nine products is a format the
/// crate stores, converts and processes, so no value of this type is invalid.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct ColorFormat {
    pub channel_count: ChannelCount,
    pub sample_type: SampleType,
}

impl ChannelCount {
    pub const fn count(self) -> usize {
        self as usize
    }
}

impl SampleType {
    /// Bytes one channel value occupies.
    pub const fn size(self) -> usize {
        match self {
            Self::U8 => 1,
            Self::U16 => 2,
            Self::F32 => 4,
        }
    }

    pub const fn bits(self) -> usize {
        self.size() * 8
    }

    pub const fn is_float(self) -> bool {
        matches!(self, Self::F32)
    }
}

impl ColorFormat {
    pub const L_U8: Self = Self::new(ChannelCount::L, SampleType::U8);
    pub const L_U16: Self = Self::new(ChannelCount::L, SampleType::U16);
    pub const L_F32: Self = Self::new(ChannelCount::L, SampleType::F32);
    pub const RGB_U8: Self = Self::new(ChannelCount::Rgb, SampleType::U8);
    pub const RGB_U16: Self = Self::new(ChannelCount::Rgb, SampleType::U16);
    pub const RGB_F32: Self = Self::new(ChannelCount::Rgb, SampleType::F32);
    pub const RGBA_U8: Self = Self::new(ChannelCount::Rgba, SampleType::U8);
    pub const RGBA_U16: Self = Self::new(ChannelCount::Rgba, SampleType::U16);
    pub const RGBA_F32: Self = Self::new(ChannelCount::Rgba, SampleType::F32);

    pub const fn new(channel_count: ChannelCount, sample_type: SampleType) -> Self {
        Self {
            channel_count,
            sample_type,
        }
    }

    /// Bytes one pixel occupies.
    pub const fn byte_count(self) -> usize {
        self.channel_count.count() * self.sample_type.size()
    }

    /// Whether the last channel is alpha.
    pub const fn has_alpha(self) -> bool {
        matches!(self.channel_count, ChannelCount::Rgba)
    }
}

impl fmt::Display for ChannelCount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::L => "L",
            Self::Rgb => "RGB",
            Self::Rgba => "RGBA",
        })
    }
}

impl fmt::Display for SampleType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::U8 => "u8",
            Self::U16 => "u16",
            Self::F32 => "f32",
        })
    }
}

impl fmt::Display for ColorFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.channel_count, self.sample_type)
    }
}

/// Every color format.
pub const ALL_FORMATS: [ColorFormat; 9] = [
    ColorFormat::L_U8,
    ColorFormat::L_U16,
    ColorFormat::L_F32,
    ColorFormat::RGB_U8,
    ColorFormat::RGB_U16,
    ColorFormat::RGB_F32,
    ColorFormat::RGBA_U8,
    ColorFormat::RGBA_U16,
    ColorFormat::RGBA_F32,
];
