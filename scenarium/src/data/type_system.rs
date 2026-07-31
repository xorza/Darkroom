use std::str::FromStr;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use strum::VariantNames;

use ::common::id_type;

use crate::StaticValue;

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
    /// deliberately exhaustive: a new [`StaticValue`] variant must fail to
    /// compile here rather than fall into `Any` unexamined.
    pub(crate) fn or_const_type(self, value: &StaticValue) -> DataType {
        if !matches!(self, DataType::Any) {
            return self;
        }
        match value {
            StaticValue::Float(_) => DataType::Float,
            StaticValue::Int(_) => DataType::Int,
            StaticValue::Bool(_) => DataType::Bool,
            StaticValue::String(_) => DataType::String,
            // Null is typeless by nature — there is no type it could report.
            StaticValue::Null => DataType::Any,
            // A path literal is a bare string; [`DataType::FsPath`] needs the
            // mode and extension list, which only a declaration carries.
            StaticValue::FsPath(_) | StaticValue::FsPaths(_) => DataType::Any,
            // An enum literal is a variant *name*; [`DataType::Enum`] needs the
            // enum's `TypeId`, which a name cannot supply. The same asymmetry
            // [`Self::default_value`] hits in the other direction.
            StaticValue::Enum(_) => DataType::Any,
        }
    }

    pub fn default_value(&self) -> Option<StaticValue> {
        Some(match self {
            DataType::Any => StaticValue::Null,
            DataType::Float => StaticValue::Float(0.0),
            DataType::Int => StaticValue::Int(0),
            DataType::Bool => StaticValue::Bool(false),
            DataType::String => StaticValue::String(String::new()),
            DataType::FsPath(config) => match config.mode {
                FsPathMode::ExistingFiles => StaticValue::FsPaths(Vec::new()),
                FsPathMode::ExistingFile | FsPathMode::NewFile | FsPathMode::Directory => {
                    StaticValue::FsPath(String::new())
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
    /// ([`is_numeric_scalar`](Self::is_numeric_scalar)). The **literal** half
    /// is [`accepts_const`](Self::accepts_const), which reads the same class
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
    /// [`as_f64`](StaticValue::as_f64)/[`as_i64`](StaticValue::as_i64)/[`as_bool`](StaticValue::as_bool)
    /// convert freely between them — `Bool` reads as `0`/`1`, `Float`
    /// truncates to int, any nonzero reads as `true`.
    /// [`StaticValue::is_numeric_scalar`] is the same class on the value side.
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
        value: &StaticValue,
        strictness: Strictness,
        enum_variant: impl FnOnce(TypeId, &str) -> bool,
    ) -> bool {
        match self {
            DataType::Any => true,
            DataType::Float | DataType::Int | DataType::Bool => match strictness {
                Strictness::Authored => value.is_numeric_scalar(),
                Strictness::Declared => matches!(
                    (self, value),
                    (DataType::Float, StaticValue::Float(_))
                        | (DataType::Int, StaticValue::Int(_))
                        | (DataType::Bool, StaticValue::Bool(_))
                ),
            },
            DataType::String => matches!(value, StaticValue::String(_)),
            DataType::FsPath(config) => match config.mode {
                FsPathMode::ExistingFiles => matches!(value, StaticValue::FsPaths(_)),
                FsPathMode::ExistingFile | FsPathMode::NewFile | FsPathMode::Directory => {
                    matches!(value, StaticValue::FsPath(_))
                }
            },
            DataType::Enum(type_id) => {
                matches!(value, StaticValue::Enum(name) if enum_variant(*type_id, name))
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
mod tests {
    use super::*;

    #[test]
    fn type_id_from_name_is_deterministic_namespaced_and_v5() {
        let namespace_a = TypeId::from_u128(0x11);
        let namespace_b = TypeId::from_u128(0x22);

        assert_eq!(
            TypeId::from_name(namespace_a, "Mode"),
            TypeId::from_name(namespace_a, "Mode")
        );
        assert_ne!(
            TypeId::from_name(namespace_a, "Mode"),
            TypeId::from_name(namespace_a, "Other")
        );
        assert_ne!(
            TypeId::from_name(namespace_a, "Mode"),
            TypeId::from_name(namespace_b, "Mode")
        );

        let id = TypeId::from_name(namespace_a, "Mode");
        assert_eq!((id.as_u128() >> 76) & 0xf, 5);
        assert_ne!(id.as_u128() >> 64, 0);
    }

    #[test]
    fn compatibility_and_defaults_follow_runtime_coercions() {
        let custom = |id| DataType::Custom(TypeId::from_u128(id));

        assert!(DataType::Float.compatible_with(&DataType::Int));
        assert!(DataType::Bool.compatible_with(&DataType::Float));
        assert!(DataType::Any.compatible_with(&DataType::String));
        assert!(!DataType::String.compatible_with(&DataType::Bool));
        assert!(custom(1).compatible_with(&custom(1)));
        assert!(!custom(1).compatible_with(&custom(2)));

        assert_eq!(
            DataType::Float.default_value(),
            Some(StaticValue::Float(0.0))
        );
        assert_eq!(DataType::Int.default_value(), Some(StaticValue::Int(0)));
        assert_eq!(
            DataType::Bool.default_value(),
            Some(StaticValue::Bool(false))
        );
        assert_eq!(
            DataType::FsPath(Arc::new(FsPathConfig::default())).default_value(),
            Some(StaticValue::FsPath(String::new()))
        );
        assert_eq!(
            DataType::FsPath(Arc::new(FsPathConfig::new(FsPathMode::ExistingFiles)))
                .default_value(),
            Some(StaticValue::FsPaths(Vec::new()))
        );
        assert_eq!(custom(1).default_value(), None);
        assert_eq!(DataType::Enum(TypeId::from_u128(2)).default_value(), None);
    }

    /// The two strictnesses must actually diverge, and only in the two arms
    /// they are documented to: scalars and enums. Every other arm has to agree,
    /// which is the property that lets one table serve both gates.
    #[test]
    fn strictness_changes_the_scalar_and_enum_arms_and_nothing_else() {
        let mode = TypeId::from_u128(0x5ca1ab1e);
        let known = |_: TypeId, name: &str| name == "fast";
        let declared =
            |ty: &DataType, v: &StaticValue| ty.accepts_const(v, Strictness::Declared, |_, _| true);
        let authored =
            |ty: &DataType, v: &StaticValue| ty.accepts_const(v, Strictness::Authored, known);

        // Scalars: a `Bool` literal on an `Int` port is a coercion the runtime
        // performs (`as_i64` reads `true` as 1), so a document may author it —
        // and a *declaration* may not, because nobody chose it there.
        let int = DataType::Int;
        let boolean = StaticValue::Bool(true);
        assert!(
            authored(&int, &boolean),
            "the runtime reads this, so a document may write it"
        );
        assert!(
            !declared(&int, &boolean),
            "but a declared default is held to its own kind"
        );
        assert_ne!(declared(&int, &boolean), authored(&int, &boolean));

        // …and the exact kind satisfies both, so `Declared` is narrower rather
        // than merely different.
        for (ty, value) in [
            (DataType::Int, StaticValue::Int(1)),
            (DataType::Float, StaticValue::Float(1.0)),
            (DataType::Bool, StaticValue::Bool(false)),
        ] {
            assert!(
                declared(&ty, &value) && authored(&ty, &value),
                "{ty:?} takes its own kind either way"
            );
        }

        // Enums: membership is the registry's answer, so only the authored gate
        // asks it. Both still require an *enum* literal.
        let enum_ty = DataType::Enum(mode);
        let unregistered = StaticValue::Enum("slothful".into());
        let registered = StaticValue::Enum("fast".into());
        assert!(
            declared(&enum_ty, &unregistered),
            "no registry exists at declaration time"
        );
        assert!(
            !authored(&enum_ty, &unregistered),
            "an authored literal must name a variant"
        );
        assert_ne!(
            declared(&enum_ty, &unregistered),
            authored(&enum_ty, &unregistered)
        );
        assert!(declared(&enum_ty, &registered) && authored(&enum_ty, &registered));
        assert!(
            !declared(&enum_ty, &StaticValue::Int(0)),
            "an enum port takes enum literals only"
        );

        // Every other arm agrees, whichever gate asks.
        let path = DataType::FsPath(Arc::new(FsPathConfig::new(FsPathMode::ExistingFiles)));
        let cases = [
            (DataType::Any, StaticValue::Enum("anything".into()), true),
            (DataType::String, StaticValue::String("s".into()), true),
            (DataType::String, StaticValue::Int(1), false),
            (path.clone(), StaticValue::FsPaths(vec!["a".into()]), true),
            (path, StaticValue::FsPath("a".into()), false),
            (
                DataType::FsPath(Arc::new(FsPathConfig::default())),
                StaticValue::FsPath("a".into()),
                true,
            ),
            (
                DataType::Custom(mode),
                StaticValue::String("s".into()),
                false,
            ),
        ];
        for (ty, value, expected) in cases {
            assert_eq!(
                declared(&ty, &value),
                expected,
                "declared {ty:?} / {value:?}"
            );
            assert_eq!(
                authored(&ty, &value),
                expected,
                "authored {ty:?} / {value:?}"
            );
        }
    }
}
