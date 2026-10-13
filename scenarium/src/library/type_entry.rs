use std::sync::Arc;

use crate::{CustomValueCodec, EnumVariants};

#[derive(Clone, Debug)]
enum TypeEntryKind {
    Custom {
        display_name: String,
        codec: Option<Arc<dyn CustomValueCodec>>,
    },
    Enum {
        display_name: String,
        variants: Vec<String>,
    },
}

/// A registered nominal type. A custom type may carry a disk codec; an enum
/// carries only its display metadata and variants.
#[derive(Clone, Debug)]
pub struct TypeEntry {
    kind: TypeEntryKind,
}

impl TypeEntry {
    fn custom_entry(
        display_name: impl Into<String>,
        codec: Option<Arc<dyn CustomValueCodec>>,
    ) -> Self {
        Self {
            kind: TypeEntryKind::Custom {
                display_name: display_name.into(),
                codec,
            },
        }
    }

    /// A custom type with no disk codec (not cacheable).
    pub fn custom(display_name: impl Into<String>) -> Self {
        Self::custom_entry(display_name, None)
    }

    /// A custom type with a disk codec.
    pub fn custom_with_codec(
        display_name: impl Into<String>,
        codec: Arc<dyn CustomValueCodec>,
    ) -> Self {
        Self::custom_entry(display_name, Some(codec))
    }

    /// An enum type with the variant names taken from `E` (via strum).
    pub fn enum_of<E: EnumVariants>(display_name: impl Into<String>) -> Self {
        Self::enum_with_variants(display_name, E::variant_names())
    }

    /// An enum type with an explicit variant list (for runtime-discovered enums
    /// where the concrete type isn't available — see `lens`'s config builders).
    pub fn enum_with_variants(display_name: impl Into<String>, variants: Vec<String>) -> Self {
        Self {
            kind: TypeEntryKind::Enum {
                display_name: display_name.into(),
                variants,
            },
        }
    }

    pub fn display_name(&self) -> &str {
        match &self.kind {
            TypeEntryKind::Custom { display_name, .. }
            | TypeEntryKind::Enum { display_name, .. } => display_name,
        }
    }

    /// The variant names for an enum entry; `None` for a custom type.
    pub fn variants(&self) -> Option<&[String]> {
        match &self.kind {
            TypeEntryKind::Enum { variants, .. } => Some(variants),
            TypeEntryKind::Custom { .. } => None,
        }
    }

    /// Whether `other` declares this same type: the same name, the same
    /// variants, and the same codec instance.
    pub(super) fn same_as(&self, other: &Self) -> bool {
        match (&self.kind, &other.kind) {
            (
                TypeEntryKind::Custom {
                    display_name,
                    codec,
                },
                TypeEntryKind::Custom {
                    display_name: other_name,
                    codec: other_codec,
                },
            ) => {
                display_name == other_name
                    && match (codec, other_codec) {
                        (None, None) => true,
                        (Some(codec), Some(other)) => Arc::ptr_eq(codec, other),
                        _ => false,
                    }
            }
            (
                TypeEntryKind::Enum {
                    display_name,
                    variants,
                },
                TypeEntryKind::Enum {
                    display_name: other_name,
                    variants: other_variants,
                },
            ) => display_name == other_name && variants == other_variants,
            _ => false,
        }
    }

    pub(super) fn codec(&self) -> Option<&Arc<dyn CustomValueCodec>> {
        match &self.kind {
            TypeEntryKind::Custom { codec, .. } => codec.as_ref(),
            TypeEntryKind::Enum { .. } => None,
        }
    }
}
