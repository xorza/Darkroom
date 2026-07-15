use std::str::FromStr;
use std::sync::LazyLock;

use imaginarium::ColorFormat;
use scenarium::{DataType, EnumVariants, TypeId};
use strum::IntoEnumIterator;
use strum_macros::EnumIter;

#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumIter)]
pub(crate) enum ConversionFormat {
    /// Keep the image's current color format (no conversion).
    AsIs,
    LU8,
    LU16,
    LF32,
    RgbU8,
    RgbU16,
    RgbF32,
    RgbaU8,
    RgbaU16,
    RgbaF32,
}

impl ConversionFormat {
    const AS_IS_LABEL: &'static str = "As Is";

    /// The target color format, or `None` for [`ConversionFormat::AsIs`] (pass
    /// the image through unchanged).
    pub(crate) fn to_color_format(self) -> Option<ColorFormat> {
        Some(match self {
            ConversionFormat::AsIs => return None,
            ConversionFormat::LU8 => ColorFormat::L_U8,
            ConversionFormat::LU16 => ColorFormat::L_U16,
            ConversionFormat::LF32 => ColorFormat::L_F32,
            ConversionFormat::RgbU8 => ColorFormat::RGB_U8,
            ConversionFormat::RgbU16 => ColorFormat::RGB_U16,
            ConversionFormat::RgbF32 => ColorFormat::RGB_F32,
            ConversionFormat::RgbaU8 => ColorFormat::RGBA_U8,
            ConversionFormat::RgbaU16 => ColorFormat::RGBA_U16,
            ConversionFormat::RgbaF32 => ColorFormat::RGBA_F32,
        })
    }

    /// The dropdown label: the color-format string, or "As Is" for the
    /// pass-through variant.
    pub(crate) fn label(self) -> String {
        match self.to_color_format() {
            Some(fmt) => fmt.to_string(),
            None => Self::AS_IS_LABEL.to_string(),
        }
    }
}

static CONVERSION_FORMAT_VARIANTS: LazyLock<Vec<String>> = LazyLock::new(|| {
    ConversionFormat::iter()
        .map(ConversionFormat::label)
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
            .find(|f| f.label() == s)
            .ok_or_else(|| format!("Unknown conversion format: {}", s))
    }
}

pub(crate) static CONVERSION_FORMAT_TYPE_ID: LazyLock<TypeId> =
    LazyLock::new(|| "6d9db73e-5c92-4332-af0d-b2eb7c95acd0".into());

pub(crate) static CONVERSION_FORMAT_DATATYPE: LazyLock<DataType> =
    LazyLock::new(|| DataType::Enum(*CONVERSION_FORMAT_TYPE_ID));

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_is_is_a_pass_through() {
        assert_eq!(ConversionFormat::AsIs.to_color_format(), None);
        assert_eq!(ConversionFormat::AsIs.label(), "As Is");
        assert_eq!(
            ConversionFormat::from_str("As Is").unwrap(),
            ConversionFormat::AsIs,
        );
    }

    #[test]
    fn real_formats_map_and_roundtrip() {
        // Every non-`AsIs` variant has a `ColorFormat` whose Display drives both
        // its label and the `FromStr` inverse, so the dropdown string, wire value,
        // and enum stay in lockstep.
        for f in ConversionFormat::iter().filter(|f| *f != ConversionFormat::AsIs) {
            let fmt = f.to_color_format().expect("real format has a ColorFormat");
            assert_eq!(f.label(), fmt.to_string());
            assert_eq!(ConversionFormat::from_str(&f.label()).unwrap(), f);
        }
        // Spot-check one concrete mapping so a Display change can't silently drift.
        assert_eq!(ConversionFormat::RgbU8.label(), "RGB u8");
    }

    #[test]
    fn as_is_leads_the_variant_list() {
        // `enum_input` seeds a dropdown to its first variant, so "As Is" first is
        // what makes Save Image default to no conversion.
        let names = ConversionFormat::variant_names();
        assert_eq!(names.first().map(String::as_str), Some("As Is"));
        assert_eq!(names.len(), ConversionFormat::iter().count());
    }

    #[test]
    fn from_str_rejects_unknown() {
        assert!(ConversionFormat::from_str("not a format").is_err());
    }
}
