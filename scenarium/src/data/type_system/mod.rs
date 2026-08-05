use std::str::FromStr;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use strum::VariantNames;

use ::common::id_type;

use crate::ConstValue;

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

id_type!(TypeId);

#[cfg(test)]
impl TypeId {
    fn from_name(namespace: TypeId, name: &str) -> Self {
        uuid::Uuid::new_v5(&namespace.as_uuid(), name.as_bytes()).into()
    }
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FsPathMode {
    #[default]
    ExistingFile,
    ExistingFiles,
    NewFile,
    Directory,
}

#[derive(Clone, Default, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FsPathConfig {
    pub mode: FsPathMode,
    pub extensions: Vec<String>,
}

impl FsPathConfig {
    pub fn new(mode: FsPathMode) -> Self {
        Self {
            mode,
            extensions: Vec::new(),
        }
    }

    pub fn with_extensions(mode: FsPathMode, extensions: Vec<String>) -> Self {
        Self { mode, extensions }
    }
}

#[derive(Clone, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataType {
    #[default]
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
    /// This type when it is definite, or the type `value` itself carries when it
    /// is `Any`.
    ///
    /// The rule a wildcard output resolves a *constant* through: a declared type
    /// wins, because it carries what a literal cannot — an `FsPathConfig`, an
    /// enum's id. Only a polymorphic declaration falls back to the literal.
    ///
    /// **`Any` here means "the literal cannot say", not "unhandled".** Reaching
    /// it takes nothing but an authored const on an `Any`-declared input, so it
    /// is ordinary data rather than a broken contract, and the arms below spell
    /// out which literals describe no type of their own and why. The match is
    /// deliberately exhaustive: a new [`ConstValue`] variant must fail to
    /// compile here rather than fall into `Any` unexamined.
    pub(crate) fn or_const_type(self, value: &ConstValue) -> DataType {
        if !matches!(self, DataType::Any) {
            return self;
        }
        match value {
            ConstValue::Float(_) => DataType::Float,
            ConstValue::Int(_) => DataType::Int,
            ConstValue::Bool(_) => DataType::Bool,
            ConstValue::String(_) => DataType::String,
            // Null is typeless by nature — there is no type it could report.
            ConstValue::Null => DataType::Any,
            // A path literal is a bare string; [`DataType::FsPath`] needs the
            // mode and extension list, which only a declaration carries.
            ConstValue::FsPath(_) | ConstValue::FsPaths(_) => DataType::Any,
            // An enum literal is a variant *name*; [`DataType::Enum`] needs the
            // enum's `TypeId`, which a name cannot supply. The same asymmetry
            // [`Self::default_value`] hits in the other direction.
            ConstValue::Enum(_) => DataType::Any,
        }
    }

    pub fn default_value(&self) -> Option<ConstValue> {
        Some(match self {
            DataType::Any => ConstValue::Null,
            DataType::Float => ConstValue::Float(0.0),
            DataType::Int => ConstValue::Int(0),
            DataType::Bool => ConstValue::Bool(false),
            DataType::String => ConstValue::String(String::new()),
            DataType::FsPath(config) => match config.mode {
                FsPathMode::ExistingFiles => ConstValue::FsPaths(Vec::new()),
                FsPathMode::ExistingFile | FsPathMode::NewFile | FsPathMode::Directory => {
                    ConstValue::FsPath(String::new())
                }
            },
            DataType::Custom(_) | DataType::Enum(_) => return None,
        })
    }

    /// Whether a value of type `source` may feed a port of type `self` — the
    /// **wire** half of the compile-boundary gate.
    ///
    /// Deliberately looser than type equality: `Any` accepts everything, and
    /// the three scalar kinds form one coercion class
    /// (`is_numeric_scalar`). The **literal** half
    /// is `accepts_const`, which reads the same class
    /// off the value side — so wiring and constants are exactly as permissive
    /// as what executes.
    pub fn compatible_with(&self, source: &DataType) -> bool {
        if matches!(self, DataType::Any) || matches!(source, DataType::Any) {
            return true;
        }
        if self.is_numeric_scalar() && source.is_numeric_scalar() {
            return true;
        }
        self == source
    }

    /// The scalar coercion class, type side: `Float`/`Int`/`Bool` are one kind
    /// to a runtime read, since
    /// [`as_f64`](ConstValue::as_f64)/[`as_i64`](ConstValue::as_i64)/[`as_bool`](ConstValue::as_bool)
    /// convert freely between them — `Bool` reads as `0`/`1`, `Float`
    /// truncates to int, any nonzero reads as `true`.
    /// [`ConstValue::is_numeric_scalar`] is the same class on the value side.
    fn is_numeric_scalar(&self) -> bool {
        matches!(self, DataType::Float | DataType::Int | DataType::Bool)
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
            DataType::Any => true,
            DataType::Float | DataType::Int | DataType::Bool => match strictness {
                Strictness::Authored => value.is_numeric_scalar(),
                Strictness::Declared => matches!(
                    (self, value),
                    (DataType::Float, ConstValue::Float(_))
                        | (DataType::Int, ConstValue::Int(_))
                        | (DataType::Bool, ConstValue::Bool(_))
                ),
            },
            DataType::String => matches!(value, ConstValue::String(_)),
            DataType::FsPath(config) => match config.mode {
                FsPathMode::ExistingFiles => matches!(value, ConstValue::FsPaths(_)),
                FsPathMode::ExistingFile | FsPathMode::NewFile | FsPathMode::Directory => {
                    matches!(value, ConstValue::FsPath(_))
                }
            },
            DataType::Enum(type_id) => {
                matches!(value, ConstValue::Enum(name) if enum_variant(*type_id, name))
            }
            DataType::Custom(_) => false,
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

impl FromStr for DataType {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "float" | "number" => Ok(DataType::Float),
            "int" => Ok(DataType::Int),
            "bool" => Ok(DataType::Bool),
            "string" => Ok(DataType::String),
            "path" => Ok(DataType::FsPath(Arc::new(FsPathConfig::default()))),
            _ => Err(()),
        }
    }
}

#[cfg(test)]
mod tests;
