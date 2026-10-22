use std::sync::Arc;

use strum::VariantNames;

use common::Id;

use crate::data::const_value::ConstValue;

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

/// The id of the custom type or enum a [`DataType::Custom`] or [`DataType::Enum`] names.
pub type TypeId = Id<DataType>;

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Hash)]
pub enum FsPathMode {
    #[default]
    ExistingFile,
    ExistingFiles,
    NewFile,
    Dir,
}

#[derive(Clone, Default, Debug, PartialEq, Eq, Hash)]
pub struct FsPathConfig {
    pub mode: FsPathMode,
    pub extensions: Vec<String>,
}

impl FsPathConfig {
    pub const fn new(mode: FsPathMode) -> Self {
        Self {
            mode,
            extensions: Vec::new(),
        }
    }

    pub const fn with_extensions(mode: FsPathMode, extensions: Vec<String>) -> Self {
        Self { mode, extensions }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DataType {
    Any,
    Float,
    Int,
    Bool,
    String,
    FsPath(Arc<FsPathConfig>),
    Custom(TypeId),
    Enum(TypeId),
}

impl DataType {
    pub fn default_value(&self) -> Option<ConstValue> {
        Some(match self {
            Self::Any => ConstValue::Null,
            Self::Float => ConstValue::Float(0.0),
            Self::Int => ConstValue::Int(0),
            Self::Bool => ConstValue::Bool(false),
            Self::String => ConstValue::String(String::new()),
            Self::FsPath(config) => match config.mode {
                FsPathMode::ExistingFiles => ConstValue::FsPaths(Vec::new()),
                FsPathMode::ExistingFile | FsPathMode::NewFile | FsPathMode::Dir => {
                    ConstValue::FsPath(String::new())
                }
            },
            Self::Custom(_) | Self::Enum(_) => return None,
        })
    }

    /// Whether a value of type `source` may feed a port of type `self` — the
    /// **wire** half of the compile-boundary gate.
    ///
    /// Deliberately looser than type equality: an `Any` input accepts
    /// everything, and the three scalar kinds form one coercion class
    /// (`is_numeric_scalar`). The **literal** half
    /// is `accepts_const`, which reads the same class
    /// off the value side — so wiring and constants are exactly as permissive
    /// as what executes.
    pub(crate) fn compatible_with(&self, source: &Self) -> bool {
        if matches!(self, Self::Any) {
            return true;
        }
        if self.is_numeric_scalar() && source.is_numeric_scalar() {
            return true;
        }
        self == source
    }

    /// The scalar coercion class, type side: `Float`/`Int`/`Bool` are one kind to a runtime read,
    /// since
    /// [`as_f64`](ConstValue::as_f64)/[`as_i64`](ConstValue::as_i64)/[`as_bool`](ConstValue::as_bool)
    /// convert freely between them — `Bool` reads as `0`/`1`, `Float` truncates to int, any nonzero
    /// reads as `true`. [`ConstValue::is_numeric_scalar`] is the same class on the value side.
    const fn is_numeric_scalar(&self) -> bool {
        matches!(self, Self::Float | Self::Int | Self::Bool)
    }

    /// Whether `value` is a well-formed literal for a port declared `self` —
    /// **the** table, read by both const gates.
    ///
    /// The two differ in exactly the two arms below, and nowhere else, which
    /// is why they are one function with a knob rather than two matches to
    /// keep in step:
    ///
    /// - **Scalars.** [`Strictness::Authored`] takes the whole coercion class,
    ///   so a document's literal is as permissive as the runtime read that
    ///   consumes it. [`Strictness::Declared`] demands the exact kind, so a
    ///   `Bool` default on an `Int` port is an authoring slip caught at
    ///   registration rather than a coercion nobody chose.
    /// - **Enums.** A literal is a variant *name*; whether that name is
    ///   registered is a question only a [`Library`](crate::Library) can
    ///   answer, so the caller supplies `enum_variant`. Declaration time has
    ///   no registry yet and passes `|_, _| true` — `Library::register_type`
    ///   re-checks declared defaults once the type arrives.
    ///
    /// A `Custom` port has no literal form at all, and `Any` takes anything.
    pub(crate) fn accepts_const(
        &self,
        value: &ConstValue,
        strictness: Strictness,
        enum_variant: impl FnOnce(TypeId, &str) -> bool,
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
            Self::FsPath(config) => match config.mode {
                FsPathMode::ExistingFiles => matches!(value, ConstValue::FsPaths(_)),
                FsPathMode::ExistingFile | FsPathMode::NewFile | FsPathMode::Dir => {
                    matches!(value, ConstValue::FsPath(_))
                }
            },
            Self::Enum(type_id) => {
                matches!(value, ConstValue::Enum(name) if enum_variant(*type_id, name))
            }
            Self::Custom(_) => false,
        }
    }
}

/// Which const gate is asking [`DataType::accepts_const`] — the whole of what
/// separates the two.
///
/// They are not the same question asked twice: one polices a *declaration* a
/// programmer wrote, before any document exists; the other polices a *literal*
/// a user authored, against the same library a run will use. Held apart so the
/// looser one cannot be applied where the stricter was meant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Strictness {
    /// A `Func`'s own declared default, at registration.
    Declared,
    /// A document's authored `Const`, at compile.
    Authored,
}

#[cfg(test)]
mod tests;
