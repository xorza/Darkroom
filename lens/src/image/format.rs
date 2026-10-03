use imaginarium::{ALL_FORMATS, ColorFormat};
use std::str::FromStr;

use scenarium::{DataType, EnumVariants, TypeId};

/// The `Format` variant that keeps the image's color format.
pub(super) const AS_IS: &str = "As Is";

/// The `Format` port's variants: [`AS_IS`], then every color format by name.
const VARIANTS: [&str; ALL_FORMATS.len() + 1] = {
    let mut variants = [AS_IS; ALL_FORMATS.len() + 1];
    let mut index = 0;
    while index < ALL_FORMATS.len() {
        variants[index + 1] = ALL_FORMATS[index].name();
        index += 1;
    }
    variants
};

/// What a `Format` port picks: keep the image's color format, or convert to
/// one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ConversionFormat {
    AsIs,
    To(ColorFormat),
}

impl ConversionFormat {
    /// The format to convert an image in `current` to, or `None` when it
    /// stays as it is.
    pub(super) fn target(self, current: ColorFormat) -> Option<ColorFormat> {
        match self {
            Self::AsIs => None,
            Self::To(format) => (format != current).then_some(format),
        }
    }
}

impl FromStr for ConversionFormat {
    type Err = ();

    /// One of the port's variant names: [`AS_IS`], or a color format's name.
    fn from_str(name: &str) -> Result<Self, ()> {
        if name == AS_IS {
            return Ok(Self::AsIs);
        }
        ALL_FORMATS
            .into_iter()
            .find(|format| format.name() == name)
            .map(Self::To)
            .ok_or(())
    }
}

impl EnumVariants for ConversionFormat {
    fn variant_names() -> Vec<String> {
        VARIANTS.map(String::from).to_vec()
    }
}

pub(super) const CONVERSION_FORMAT_TYPE_ID: TypeId =
    TypeId::literal("6d9db73e-5c92-4332-af0d-b2eb7c95acd0");

pub(super) const CONVERSION_FORMAT_DATATYPE: DataType = DataType::Enum(CONVERSION_FORMAT_TYPE_ID);

#[cfg(test)]
mod tests {
    use imaginarium::{ALL_FORMATS, ColorFormat};
    use scenarium::EnumVariants;

    use crate::image::format::{AS_IS, ConversionFormat};

    /// `enum_input` seeds a dropdown to its first variant, so "As Is" first is what makes Save
    /// Image default to no conversion; every color format follows by name, and every variant
    /// parses back to what it names.
    #[test]
    fn as_is_leads_every_format_name() {
        let names = ConversionFormat::variant_names();
        assert_eq!(names[0], AS_IS);
        assert_eq!(names[0], "As Is");
        assert_eq!(names.len(), 10);
        assert_eq!(names[0].parse(), Ok(ConversionFormat::AsIs));
        for (name, format) in names[1..].iter().zip(ALL_FORMATS) {
            assert_eq!(name, &format.to_string());
            assert_eq!(name.parse(), Ok(ConversionFormat::To(format)));
        }
        assert_eq!("RGB u9".parse::<ConversionFormat>(), Err(()));
    }

    #[test]
    fn as_is_and_the_current_format_convert_nothing() {
        assert_eq!(ConversionFormat::AsIs.target(ColorFormat::RGB_U8), None);
        let to_rgb = ConversionFormat::To(ColorFormat::RGB_U8);
        assert_eq!(to_rgb.target(ColorFormat::RGB_U8), None);
        assert_eq!(
            to_rgb.target(ColorFormat::RGBA_F32),
            Some(ColorFormat::RGB_U8)
        );
    }
}
