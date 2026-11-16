//! [`DataType`]: what a port accepts, a small value of ids into the library's registry.
//!
//! # Decisions
//!
//! - **A type names what it accepts and holds ids alone.** A scalar, a directory, or a reference
//!   into the [`Library`](crate::Library)'s registry: an enum, a custom type or a file kind. As
//!   MaterialX, whose types are `typedef` elements of the library, a port's `type` being a
//!   typedef's name and `filename` one of them
//!   ([specification](https://github.com/AcademySoftwareFoundation/MaterialX/blob/main/documents/Specification/MaterialX.Specification.md)).
//!   Refused: a dialog's mode and extension list inside the type behind an `Arc`, which a
//!   document then serializes, copying the list once per port.
//! - **A file type is a registered file kind and a mode.** The mode decides the kind of value,
//!   one path or several, and the dialog that picks it; the kind decides which files it holds.
//! - **Each kind of registered type has its own id type and its own table**, so the variant in a
//!   type and the entry in the registry cannot disagree. Refused: one `TypeId` for all, which
//!   needed an assert at each registration that the two agree.

pub(crate) mod custom_type;
pub(crate) mod enum_type;
pub(crate) mod file_kind;

use serde::{Deserialize, Serialize};
use strum::VariantNames;

use crate::data::const_value::ConstValue;
use crate::data::data_type::custom_type::CustomTypeId;
use crate::data::data_type::enum_type::EnumId;
use crate::data::data_type::file_kind::FileKindId;

pub trait EnumVariants {
    fn variant_names() -> Vec<String>;
}

impl<T: VariantNames> EnumVariants for T {
    fn variant_names() -> Vec<String> {
        T::VARIANTS
            .iter()
            .map(|variant| (*variant).to_string())
            .collect()
    }
}

/// How a file port takes its value: the kind of value it holds, and the dialog that picks it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FileMode {
    /// One file that exists.
    Open,
    /// One or more files that exist.
    OpenMany,
    /// One file to write, which may not exist yet.
    Save,
}

impl FileMode {
    /// Whether a value of this mode is a list of paths rather than one.
    pub const fn holds_many(self) -> bool {
        matches!(self, Self::OpenMany)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DataType {
    Any,
    Float,
    Int,
    Bool,
    String,
    /// A directory.
    Dir,
    /// A file of a registered kind, taken as `mode` says.
    File {
        kind: FileKindId,
        mode: FileMode,
    },
    Custom(CustomTypeId),
    Enum(EnumId),
}

impl DataType {
    pub const fn default_value(self) -> Option<ConstValue> {
        Some(match self {
            Self::Any => ConstValue::Null,
            Self::Float => ConstValue::Float(0.0),
            Self::Int => ConstValue::Int(0),
            Self::Bool => ConstValue::Bool(false),
            Self::String => ConstValue::String(String::new()),
            Self::File { mode, .. } if mode.holds_many() => ConstValue::FsPaths(Vec::new()),
            Self::Dir | Self::File { .. } => ConstValue::FsPath(String::new()),
            Self::Custom(_) | Self::Enum(_) => return None,
        })
    }

    /// Whether a value of this type is one path or several: what a run checks against the
    /// filesystem, and what a document resolves against its folder.
    pub(crate) const fn is_path(self) -> bool {
        matches!(self, Self::Dir | Self::File { .. })
    }

    /// Whether this type references an entry of the library's registry under the nil id, which
    /// names nothing.
    pub(crate) const fn has_nil_id(self) -> bool {
        match self {
            Self::File { kind, .. } => kind.is_nil(),
            Self::Custom(type_id) => type_id.is_nil(),
            Self::Enum(enum_id) => enum_id.is_nil(),
            Self::Any | Self::Float | Self::Int | Self::Bool | Self::String | Self::Dir => false,
        }
    }

    /// Whether a value of type `source` may feed a port of type `self`: the **wire** half of the
    /// compile-boundary gate. Looser than type equality: an `Any` input accepts everything, the
    /// three scalar kinds form one coercion class (`is_numeric_scalar`), and a file feeds a file
    /// input of the same kind of value whose kind holds every file the source's does. The
    /// **literal** half is `accepts_const`, which reads the same classes off the value side.
    ///
    /// Only a [`Library`](crate::Library) knows what a file kind holds, so the caller answers
    /// `within(source_kind, input_kind)`.
    pub(crate) fn compatible_with(
        self,
        source: Self,
        within: impl FnOnce(FileKindId, FileKindId) -> bool,
    ) -> bool {
        match (self, source) {
            (Self::Any, _) => true,
            (input, source) if input.is_numeric_scalar() && source.is_numeric_scalar() => true,
            (
                Self::File { kind, mode },
                Self::File {
                    kind: source_kind,
                    mode: source_mode,
                },
            ) => mode.holds_many() == source_mode.holds_many() && within(source_kind, kind),
            (input, source) => input == source,
        }
    }

    /// The scalar coercion class, type side: `Float`/`Int`/`Bool` are one kind to a runtime read,
    /// since [`as_f64`](ConstValue::as_f64), [`as_i64`](ConstValue::as_i64) and
    /// [`as_bool`](ConstValue::as_bool) convert freely between them.
    /// [`ConstValue::is_numeric_scalar`] is the same class on the value side.
    const fn is_numeric_scalar(self) -> bool {
        matches!(self, Self::Float | Self::Int | Self::Bool)
    }

    /// Whether `value` is a well-formed literal for a port declared `self`: **the** table, read
    /// by both const gates, which differ in exactly two arms:
    ///
    /// - **Scalars.** [`Strictness::Authored`] takes the whole coercion class, as permissive as
    ///   the runtime read that consumes it. [`Strictness::Declared`] demands the exact kind, so a
    ///   `Bool` default on an `Int` port is caught at registration.
    /// - **Enums.** A literal is a variant *name*; only a [`Library`](crate::Library) knows
    ///   whether it is registered, so the caller supplies `enum_variant`. Declaration time has no
    ///   registry yet and passes `|_, _| true`; `Library::check` checks declared defaults once
    ///   the library is whole.
    ///
    /// A path is checked for its kind of value alone, one path or several, not for its
    /// extension: extensions vary in case and some files have none. A `Custom` port has no
    /// literal form at all, and `Any` takes anything.
    pub(crate) fn accepts_const(
        self,
        value: &ConstValue,
        strictness: Strictness,
        enum_variant: impl FnOnce(EnumId, &str) -> bool,
    ) -> bool {
        match self {
            Self::Any => true,
            Self::Float | Self::Int | Self::Bool => match strictness {
                Strictness::Authored => value.is_numeric_scalar(),
                Strictness::Declared => matches!(
                    (self, value),
                    (Self::Float, ConstValue::Float(_))
                        | (Self::Int, ConstValue::Int(_))
                        | (Self::Bool, ConstValue::Bool(_))
                ),
            },
            Self::String => matches!(value, ConstValue::String(_)),
            Self::File { mode, .. } if mode.holds_many() => {
                matches!(value, ConstValue::FsPaths(_))
            }
            Self::Dir | Self::File { .. } => matches!(value, ConstValue::FsPath(_)),
            Self::Enum(enum_id) => {
                matches!(value, ConstValue::Enum(name) if enum_variant(enum_id, name))
            }
            Self::Custom(_) => false,
        }
    }
}

/// Which const gate is asking [`DataType::accepts_const`]: one polices a *declaration* a
/// programmer wrote, before any document exists; the other a *literal* a user authored, against
/// the library a run will use. Held apart so the looser cannot be applied where the stricter was
/// meant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Strictness {
    /// A declared default, at registration: a func's.
    Declared,
    /// A document's authored `Const`, at compile.
    Authored,
}

#[cfg(test)]
mod tests;
