use std::fmt;

/// How many interleaved channels a pixel holds.
#[derive(Debug, Hash, PartialEq, Eq, Copy, Clone)]
#[repr(u8)]
pub enum ChannelCount {
    Mono = 1,
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
    pub const fn byte_len(self) -> usize {
        match self {
            Self::U8 => 1,
            Self::U16 => 2,
            Self::F32 => 4,
        }
    }

    pub const fn bits(self) -> usize {
        self.byte_len() * 8
    }

    pub const fn is_float(self) -> bool {
        matches!(self, Self::F32)
    }
}

impl ColorFormat {
    pub const MONO_U8: Self = Self::new(ChannelCount::Mono, SampleType::U8);
    pub const MONO_U16: Self = Self::new(ChannelCount::Mono, SampleType::U16);
    pub const MONO_F32: Self = Self::new(ChannelCount::Mono, SampleType::F32);
    pub const RGB_U8: Self = Self::new(ChannelCount::Rgb, SampleType::U8);
    pub const RGB_U16: Self = Self::new(ChannelCount::Rgb, SampleType::U16);
    pub const RGB_F32: Self = Self::new(ChannelCount::Rgb, SampleType::F32);
    pub const RGBA_U8: Self = Self::new(ChannelCount::Rgba, SampleType::U8);
    pub const RGBA_U16: Self = Self::new(ChannelCount::Rgba, SampleType::U16);
    pub const RGBA_F32: Self = Self::new(ChannelCount::Rgba, SampleType::F32);

    /// Every color format.
    pub const ALL: [Self; 9] = [
        Self::MONO_U8,
        Self::MONO_U16,
        Self::MONO_F32,
        Self::RGB_U8,
        Self::RGB_U16,
        Self::RGB_F32,
        Self::RGBA_U8,
        Self::RGBA_U16,
        Self::RGBA_F32,
    ];

    pub const fn new(channel_count: ChannelCount, sample_type: SampleType) -> Self {
        Self {
            channel_count,
            sample_type,
        }
    }

    /// Bytes one pixel occupies.
    pub const fn byte_len(self) -> usize {
        self.channel_count.count() * self.sample_type.byte_len()
    }

    /// Whether the last channel is alpha.
    pub const fn has_alpha(self) -> bool {
        matches!(self.channel_count, ChannelCount::Rgba)
    }

    /// The channel layout and the sample type, as in `"RGB u8"`; the [`Display`](fmt::Display)
    /// form, available in a `const`.
    pub const fn name(self) -> &'static str {
        match (self.channel_count, self.sample_type) {
            (ChannelCount::Mono, SampleType::U8) => "Mono u8",
            (ChannelCount::Mono, SampleType::U16) => "Mono u16",
            (ChannelCount::Mono, SampleType::F32) => "Mono f32",
            (ChannelCount::Rgb, SampleType::U8) => "RGB u8",
            (ChannelCount::Rgb, SampleType::U16) => "RGB u16",
            (ChannelCount::Rgb, SampleType::F32) => "RGB f32",
            (ChannelCount::Rgba, SampleType::U8) => "RGBA u8",
            (ChannelCount::Rgba, SampleType::U16) => "RGBA u16",
            (ChannelCount::Rgba, SampleType::F32) => "RGBA f32",
        }
    }
}

impl fmt::Display for ChannelCount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Mono => "Mono",
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
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use crate::color::color_format::ColorFormat;

    /// Each name is the channel layout's and the sample type's own text, so no two match.
    #[test]
    fn name_joins_the_channel_layout_and_the_sample_type() {
        for format in ColorFormat::ALL {
            assert_eq!(
                format.name(),
                format!("{} {}", format.channel_count, format.sample_type)
            );
            assert_eq!(format.to_string(), format.name());
        }
        assert_eq!(ColorFormat::ALL[3].name(), "RGB u8");
    }
}
