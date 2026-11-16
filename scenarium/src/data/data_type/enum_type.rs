//! [`EnumType`]: a registered enum, its name and its variants.

use common::Id;

use crate::data::data_type::EnumVariants;

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
    pub fn new(name: impl Into<String>, variants: Vec<String>) -> Self {
        Self {
            name: name.into(),
            variants,
        }
    }

    /// An enum named `name` of `E`'s variant names (via strum).
    pub fn of<E: EnumVariants>(name: impl Into<String>) -> Self {
        Self::new(name, E::variant_names())
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
