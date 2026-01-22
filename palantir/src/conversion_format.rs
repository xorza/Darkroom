use std::str::FromStr;
use std::sync::LazyLock;

use imaginarium::ColorFormat;
use scenarium::data::{DataType, EnumVariants};
use strum::IntoEnumIterator;
use strum_macros::EnumIter;

#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumIter)]
pub enum ConversionFormat {
    GrayU8,
    GrayU16,
    GrayF32,
    GrayAlphaU8,
    GrayAlphaU16,
    GrayAlphaF32,
    RgbU8,
    RgbU16,
    RgbF32,
    RgbaU8,
    RgbaU16,
    RgbaF32,
}

impl ConversionFormat {
    pub fn to_color_format(self) -> ColorFormat {
        match self {
            ConversionFormat::GrayU8 => ColorFormat::GRAY_U8,
            ConversionFormat::GrayU16 => ColorFormat::GRAY_U16,
            ConversionFormat::GrayF32 => ColorFormat::GRAY_F32,
            ConversionFormat::GrayAlphaU8 => ColorFormat::GRAY_ALPHA_U8,
            ConversionFormat::GrayAlphaU16 => ColorFormat::GRAY_ALPHA_U16,
            ConversionFormat::GrayAlphaF32 => ColorFormat::GRAY_ALPHA_F32,
            ConversionFormat::RgbU8 => ColorFormat::RGB_U8,
            ConversionFormat::RgbU16 => ColorFormat::RGB_U16,
            ConversionFormat::RgbF32 => ColorFormat::RGB_F32,
            ConversionFormat::RgbaU8 => ColorFormat::RGBA_U8,
            ConversionFormat::RgbaU16 => ColorFormat::RGBA_U16,
            ConversionFormat::RgbaF32 => ColorFormat::RGBA_F32,
        }
    }
}

static CONVERSION_FORMAT_VARIANTS: LazyLock<Vec<String>> = LazyLock::new(|| {
    ConversionFormat::iter()
        .map(|f| f.to_color_format().to_string())
        .collect()
});

impl EnumVariants for ConversionFormat {
    fn variant_names() -> Vec<String> {
        CONVERSION_FORMAT_VARIANTS.clone()
    }
}

impl FromStr for ConversionFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        ConversionFormat::iter()
            .find(|f| f.to_color_format().to_string() == s)
            .ok_or_else(|| format!("Unknown conversion format: {}", s))
    }
}

pub static CONVERSION_FORMAT_DATATYPE: LazyLock<DataType> = LazyLock::new(|| {
    DataType::from_enum::<ConversionFormat>(
        "6d9db73e-5c92-4332-af0d-b2eb7c95acd0",
        "ConversionFormat",
    )
});
