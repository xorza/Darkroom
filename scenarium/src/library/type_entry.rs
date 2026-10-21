use std::sync::Arc;

use crate::data::codec::CustomValueCodec;
use crate::data::data_type::EnumVariants;

/// What a registered type is: a custom value with an optional disk codec, or
/// an enum with its variant names.
#[derive(Clone, Debug)]
enum TypeKind {
    Custom {
        codec: Option<Arc<dyn CustomValueCodec>>,
    },
    Enum {
        variants: Vec<String>,
    },
}

/// A registered nominal type. A custom type may carry a disk codec; an enum
/// carries only its display metadata and variants.
#[derive(Clone, Debug)]
pub struct TypeEntry {
    display_name: String,
    kind: TypeKind,
}

impl TypeEntry {
    /// A custom type with no disk codec (not cacheable).
    pub fn custom(display_name: impl Into<String>) -> Self {
        Self {
            display_name: display_name.into(),
            kind: TypeKind::Custom { codec: None },
        }
    }

    /// A custom type with a disk codec.
    pub fn custom_with_codec(
        display_name: impl Into<String>,
        codec: Arc<dyn CustomValueCodec>,
    ) -> Self {
        Self {
            display_name: display_name.into(),
            kind: TypeKind::Custom { codec: Some(codec) },
        }
    }

    /// An enum type with the variant names taken from `E` (via strum).
    pub fn enum_of<E: EnumVariants>(display_name: impl Into<String>) -> Self {
        Self::enum_with_variants(display_name, E::variant_names())
    }

    /// An enum type with an explicit variant list (for runtime-discovered enums
    /// where the concrete type isn't available — see `lens`'s config builders).
    pub fn enum_with_variants(display_name: impl Into<String>, variants: Vec<String>) -> Self {
        Self {
            display_name: display_name.into(),
            kind: TypeKind::Enum { variants },
        }
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    /// The variant names for an enum entry; `None` for a custom type.
    pub fn variants(&self) -> Option<&[String]> {
        match &self.kind {
            TypeKind::Enum { variants } => Some(variants),
            TypeKind::Custom { .. } => None,
        }
    }

    /// Whether `other` declares this same type: the same name, the same
    /// variants, and the same codec instance.
    pub(super) fn same_as(&self, other: &Self) -> bool {
        self.display_name == other.display_name
            && match (&self.kind, &other.kind) {
                (TypeKind::Custom { codec }, TypeKind::Custom { codec: other }) => {
                    match (codec, other) {
                        (None, None) => true,
                        (Some(codec), Some(other)) => Arc::ptr_eq(codec, other),
                        _ => false,
                    }
                }
                (TypeKind::Enum { variants }, TypeKind::Enum { variants: other }) => {
                    variants == other
                }
                _ => false,
            }
    }

    pub(super) fn codec(&self) -> Option<&Arc<dyn CustomValueCodec>> {
        match &self.kind {
            TypeKind::Custom { codec } => codec.as_ref(),
            TypeKind::Enum { .. } => None,
        }
    }
}
