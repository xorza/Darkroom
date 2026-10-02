use imaginarium::{ALL_FORMATS, ColorFormat};
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

/// The enum a `Format` port holds.
#[derive(Debug)]
pub(super) struct ConversionFormat;

impl EnumVariants for ConversionFormat {
    fn variant_names() -> Vec<String> {
        VARIANTS.map(String::from).to_vec()
    }
}

pub(super) const CONVERSION_FORMAT_TYPE_ID: TypeId =
    TypeId::literal("6d9db73e-5c92-4332-af0d-b2eb7c95acd0");

pub(super) const CONVERSION_FORMAT_DATATYPE: DataType = DataType::Enum(CONVERSION_FORMAT_TYPE_ID);

/// The format to convert an image in `current` to, or `None` for [`AS_IS`] or `current` itself.
pub(super) fn conversion_target(format: &str, current: ColorFormat) -> Option<ColorFormat> {
    if format == AS_IS {
        return None;
    }
    let target = ALL_FORMATS
        .into_iter()
        .find(|candidate| candidate.name() == format)
        .expect("enum input is validated at the compile boundary");
    (target != current).then_some(target)
}

#[cfg(test)]
mod tests {
    use imaginarium::ALL_FORMATS;
    use scenarium::EnumVariants;

    use crate::image::format::{AS_IS, ConversionFormat};

    /// `enum_input` seeds a dropdown to its first variant, so "As Is" first is what makes Save
    /// Image default to no conversion; every color format follows by name.
    #[test]
    fn as_is_leads_every_format_name() {
        let names = ConversionFormat::variant_names();
        assert_eq!(names[0], AS_IS);
        assert_eq!(names[0], "As Is");
        assert_eq!(names.len(), 10);
        for (name, format) in names[1..].iter().zip(ALL_FORMATS) {
            assert_eq!(name, &format.to_string());
        }
    }
}
