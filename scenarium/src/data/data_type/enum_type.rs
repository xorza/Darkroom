//! [`EnumType`]: a registered enum, its name and its variants.

use common::Id;

/// The id of an enum a [`DataType::Enum`](crate::DataType::Enum) names.
pub type EnumId = Id<EnumType>;

/// An enum a library registers: what the editor shows, and the variant names a literal of it
/// may hold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnumType {
    name: String,
    variants: Vec<String>,
}

impl EnumType {
    /// An enum named `name` of the variants `variants`, in order.
    ///
    /// # Panics
    /// On an empty or a repeated variant, as [`FileKind::new`](crate::FileKind::new) on an
    /// extension: a literal names a variant by its name alone.
    pub fn new(
        name: impl Into<String>,
        variants: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self::named(name.into(), variants.into_iter().map(Into::into).collect())
    }

    fn named(name: String, variants: Vec<String>) -> Self {
        for (idx, variant) in variants.iter().enumerate() {
            assert!(!variant.is_empty(), "an enum variant is not empty");
            assert!(
                !variants[..idx].contains(variant),
                "the enum variant {variant:?} is listed twice"
            );
        }
        Self { name, variants }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn variants(&self) -> &[String] {
        &self.variants
    }

    /// Whether `name` is one of the variants.
    pub(crate) fn has_variant(&self, name: &str) -> bool {
        self.variants.iter().any(|variant| variant == name)
    }
}

#[cfg(test)]
mod tests {
    use std::panic;

    use crate::data::data_type::enum_type::EnumType;

    /// An enum keeps its variants in order; an empty or a repeated one is refused.
    #[test]
    fn an_enum_refuses_an_empty_or_a_repeated_variant() {
        let modes = EnumType::new("Mode", ["Fast", "Exact"]);
        assert_eq!(modes.variants(), ["Fast", "Exact"]);
        for variants in [&["Fast", ""][..], &["Fast", "Exact", "Fast"]] {
            assert!(
                panic::catch_unwind(|| EnumType::new("Mode", variants.iter().copied())).is_err(),
                "{variants:?}"
            );
        }
    }
}
