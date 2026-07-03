use std::any::Any;
use std::fmt::Display;
use std::str::FromStr;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use strum::VariantNames;

use common::id_type;

use crate::runtime::context::ContextManager;

/// Trait for enums that can be used with `DataType::Enum`.
pub trait EnumVariants {
    fn variant_names() -> Vec<String>;
}

/// Blanket impl for types that implement strum's `VariantNames`.
impl<T: VariantNames> EnumVariants for T {
    fn variant_names() -> Vec<String> {
        T::VARIANTS.iter().map(|s| (*s).to_string()).collect()
    }
}

id_type!(TypeId);

impl TypeId {
    /// A deterministic id derived from `name` within `namespace` (UUIDv5). For
    /// nominal types discovered at runtime — e.g. an enum reflected from a config
    /// struct — where a `uuidgen` literal isn't possible: the same name always
    /// yields the same id (so it's stable across runs), and the `namespace` keeps
    /// these from colliding with ids minted for other purposes. Use a `uuidgen`
    /// literal for the `namespace`.
    pub fn from_name(namespace: TypeId, name: &str) -> Self {
        uuid::Uuid::new_v5(&namespace.as_uuid(), name.as_bytes()).into()
    }
}

/// Trait for pending preview generation that requires polling to complete.
#[async_trait::async_trait]
pub trait PendingPreview: Send {
    /// Awaits until the preview generation completes.
    async fn wait(self: Box<Self>, ctx_manager: &mut ContextManager);
}

/// Trait for custom types that can be stored in `DynamicValue::Custom`.
/// Implementors report their registered [`TypeId`] so the library can resolve
/// the type's metadata and disk codec.
///
/// `'static` (rather than an `Any` supertrait) so the method name `type_id`
/// doesn't collide with [`Any::type_id`]; downcasting still goes through
/// [`as_any`](CustomValue::as_any).
pub trait CustomValue: Send + Sync + Display + 'static {
    fn type_id(&self) -> TypeId;
    fn gen_preview(&self, _ctx_manager: &mut ContextManager) -> Option<Box<dyn PendingPreview>> {
        None
    }
    fn as_any(&self) -> &dyn Any;
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FsPathMode {
    #[default]
    ExistingFile,
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

/// A port's type. `Custom`/`Enum` reference a *registered* nominal type by
/// [`TypeId`] — the metadata (display name, enum variants, disk codec) lives on
/// the [`Library`](crate::library::Library) type table, not inline. `FsPath` is
/// structural config (not a nominal type), so it stays inline.
#[derive(Clone, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataType {
    #[default]
    Null,
    Float,
    Int,
    Bool,
    String,
    FsPath(Arc<FsPathConfig>),
    Custom(TypeId),
    Enum(TypeId),
}

/// An authored constant: the serializable value form persisted in the graph
/// (input-port `Const` bindings, func-input defaults). Carries every value kind
/// *except* the runtime-only ones — there is no unbound state and no custom
/// payload here (those live on [`DynamicValue`], which wraps this).
#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub enum StaticValue {
    #[default]
    Null,
    Float(f64),
    Int(i64),
    Bool(bool),
    String(String),
    /// A filesystem path. The path string only — the `FsPathConfig` (picker
    /// mode + extensions) is type-level metadata and lives on `DataType::FsPath`.
    FsPath(String),
    /// An enum variant by name. The variant list is type-level metadata, keyed
    /// by the `DataType::Enum(TypeId)` id in the [`Library`](crate::library::Library).
    Enum(String),
}

impl PartialEq for StaticValue {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (StaticValue::Null, StaticValue::Null) => true,
            (StaticValue::Float(left), StaticValue::Float(right)) => {
                left.to_bits() == right.to_bits()
            }
            (StaticValue::Int(left), StaticValue::Int(right)) => left == right,
            (StaticValue::Bool(left), StaticValue::Bool(right)) => left == right,
            (StaticValue::String(left), StaticValue::String(right)) => left == right,
            (StaticValue::FsPath(left), StaticValue::FsPath(right)) => left == right,
            (StaticValue::Enum(left), StaticValue::Enum(right)) => left == right,
            _ => false,
        }
    }
}

impl Eq for StaticValue {}

impl StaticValue {
    /// Numeric coercion: `Float`/`Int`/`Bool` → `f64`, else `None`.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            StaticValue::Float(value) => Some(*value),
            StaticValue::Int(value) => Some(*value as f64),
            StaticValue::Bool(value) => Some(*value as i64 as f64),
            _ => None,
        }
    }

    /// Numeric coercion: `Int`/`Float`/`Bool` → `i64`, else `None`.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            StaticValue::Int(value) => Some(*value),
            StaticValue::Float(value) => Some(*value as i64),
            StaticValue::Bool(value) => Some(*value as i64),
            _ => None,
        }
    }

    /// Truthiness coercion: `Bool`/`Int`/`Float` → `bool`, else `None`.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            StaticValue::Bool(value) => Some(*value),
            StaticValue::Int(value) => Some(*value != 0),
            StaticValue::Float(value) => Some(value.abs() > f64::EPSILON),
            _ => None,
        }
    }

    pub fn as_string(&self) -> Option<&str> {
        match self {
            StaticValue::String(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_enum(&self) -> Option<&str> {
        match self {
            StaticValue::Enum(variant) => Some(variant),
            _ => None,
        }
    }

    pub fn as_fs_path(&self) -> Option<&str> {
        match self {
            StaticValue::FsPath(path) => Some(path),
            _ => None,
        }
    }
}

impl Display for StaticValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StaticValue::Null => write!(f, "null"),
            StaticValue::Float(v) => write!(f, "{v:.4}"),
            StaticValue::Int(v) => write!(f, "{v}"),
            StaticValue::Bool(v) => write!(f, "{v}"),
            StaticValue::String(s) => write!(f, "\"{s}\""),
            StaticValue::FsPath(path) => write!(f, "\"{path}\""),
            StaticValue::Enum(variant) => write!(f, "{variant}"),
        }
    }
}

/// A runtime value flowing through execution. Structurally "an optional,
/// possibly-custom [`StaticValue`]": [`Static`](DynamicValue::Static) carries
/// every serializable kind (defined once, on `StaticValue`), while the two
/// runtime-only states live here — [`Unbound`](DynamicValue::Unbound) (no value
/// produced yet) and [`Custom`](DynamicValue::Custom) (a non-serializable
/// `Arc<dyn CustomValue>` payload). Deliberately not `Serialize`.
#[derive(Default, Clone)]
pub enum DynamicValue {
    #[default]
    Unbound,
    Static(StaticValue),
    Custom(Arc<dyn CustomValue>),
}

impl std::fmt::Debug for DynamicValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DynamicValue::Unbound => write!(f, "Unbound"),
            DynamicValue::Static(value) => write!(f, "{value:?}"),
            DynamicValue::Custom(data) => f
                .debug_struct("Custom")
                .field("type_id", &data.type_id())
                .finish_non_exhaustive(),
        }
    }
}

impl DynamicValue {
    pub fn from_custom<T: CustomValue + 'static>(value: T) -> Self {
        DynamicValue::Custom(Arc::new(value))
    }

    /// The wrapped [`StaticValue`], or `None` when unbound/custom.
    pub fn as_static(&self) -> Option<&StaticValue> {
        match self {
            DynamicValue::Static(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        self.as_static().and_then(StaticValue::as_f64)
    }

    pub fn as_i64(&self) -> Option<i64> {
        self.as_static().and_then(StaticValue::as_i64)
    }

    pub fn as_bool(&self) -> Option<bool> {
        self.as_static().and_then(StaticValue::as_bool)
    }

    pub fn as_string(&self) -> Option<&str> {
        self.as_static().and_then(StaticValue::as_string)
    }

    pub fn as_enum(&self) -> Option<&str> {
        self.as_static().and_then(StaticValue::as_enum)
    }

    pub fn as_fs_path(&self) -> Option<&str> {
        self.as_static().and_then(StaticValue::as_fs_path)
    }

    pub fn as_custom<T: CustomValue>(&self) -> Option<&T> {
        match self {
            DynamicValue::Custom(data) => data.as_any().downcast_ref::<T>(),
            _ => None,
        }
    }

    pub fn gen_preview(
        &mut self,
        ctx_manager: &mut ContextManager,
    ) -> Option<Box<dyn PendingPreview>> {
        match self {
            DynamicValue::Custom(data) => data.gen_preview(ctx_manager),
            _ => None,
        }
    }
}

impl From<&StaticValue> for DynamicValue {
    fn from(value: &StaticValue) -> Self {
        DynamicValue::Static(value.clone())
    }
}

impl From<StaticValue> for DynamicValue {
    fn from(value: StaticValue) -> Self {
        DynamicValue::Static(value)
    }
}

impl From<i64> for StaticValue {
    fn from(value: i64) -> Self {
        StaticValue::Int(value)
    }
}

impl From<i32> for StaticValue {
    fn from(value: i32) -> Self {
        StaticValue::Int(value as i64)
    }
}

impl From<f32> for StaticValue {
    fn from(value: f32) -> Self {
        StaticValue::Float(value as f64)
    }
}

impl From<f64> for StaticValue {
    fn from(value: f64) -> Self {
        StaticValue::Float(value)
    }
}

impl From<&str> for StaticValue {
    fn from(value: &str) -> Self {
        StaticValue::String(value.to_string())
    }
}

impl From<String> for StaticValue {
    fn from(value: String) -> Self {
        StaticValue::String(value)
    }
}

impl From<bool> for StaticValue {
    fn from(value: bool) -> Self {
        StaticValue::Bool(value)
    }
}

// Primitive -> DynamicValue routes through StaticValue so the shared kinds stay
// defined in one place.
impl From<i64> for DynamicValue {
    fn from(value: i64) -> Self {
        DynamicValue::Static(value.into())
    }
}

impl From<i32> for DynamicValue {
    fn from(value: i32) -> Self {
        DynamicValue::Static(value.into())
    }
}

impl From<f32> for DynamicValue {
    fn from(value: f32) -> Self {
        DynamicValue::Static(value.into())
    }
}

impl From<f64> for DynamicValue {
    fn from(value: f64) -> Self {
        DynamicValue::Static(value.into())
    }
}

impl From<&str> for DynamicValue {
    fn from(value: &str) -> Self {
        DynamicValue::Static(value.into())
    }
}

impl From<String> for DynamicValue {
    fn from(value: String) -> Self {
        DynamicValue::Static(value.into())
    }
}

impl From<bool> for DynamicValue {
    fn from(value: bool) -> Self {
        DynamicValue::Static(value.into())
    }
}

impl DataType {
    /// The zero/default constant for this type. `None` for `Custom` (no
    /// authorable literal) and `Enum` (the first-variant default needs the
    /// library's registered variant list — see
    /// [`Library`](crate::library::Library)); callers that have an enum's
    /// variants seed the default themselves.
    pub fn default_value(&self) -> Option<StaticValue> {
        Some(match self {
            DataType::Null => StaticValue::Null,
            DataType::Float => StaticValue::Float(0.0),
            DataType::Int => StaticValue::Int(0),
            DataType::Bool => StaticValue::Bool(false),
            DataType::String => StaticValue::String(String::new()),
            DataType::FsPath(_) => StaticValue::FsPath(String::new()),
            DataType::Custom(_) | DataType::Enum(_) => return None,
        })
    }
}

impl Display for DynamicValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DynamicValue::Unbound => write!(f, "-"),
            DynamicValue::Static(value) => write!(f, "{value}"),
            DynamicValue::Custom(data) => write!(f, "{data}"),
        }
    }
}

impl DataType {
    /// Whether a value of type `source` may feed a port of type `self`. The
    /// single rule the editor uses to gate connections and
    /// [`Graph::check_with`](crate::graph::Graph::check_with) uses to validate
    /// wired bindings at the compile boundary, so it must permit exactly what
    /// the engine can execute:
    /// - `Null` is the wildcard — a polymorphic passthrough / reroute port (see
    ///   [`OutputType::Wildcard`](crate::function::OutputType)) — and matches
    ///   anything on either side.
    /// - the scalar numerics (`Float`/`Int`/`Bool`) coerce among themselves at
    ///   runtime (`DynamicValue::as_f64`/`as_i64`/`as_bool` all accept the three),
    ///   so they're mutually compatible.
    /// - everything else must match exactly (derived `PartialEq`: `Custom`/`Enum`
    ///   by id, `FsPath` by config).
    pub fn compatible_with(&self, source: &DataType) -> bool {
        if matches!(self, DataType::Null) || matches!(source, DataType::Null) {
            return true;
        }
        if self.is_numeric_scalar() && source.is_numeric_scalar() {
            return true;
        }
        self == source
    }

    /// One of the runtime-coercible scalar numerics (`Float`/`Int`/`Bool`).
    fn is_numeric_scalar(&self) -> bool {
        matches!(self, DataType::Float | DataType::Int | DataType::Bool)
    }
}

impl FromStr for DataType {
    type Err = ();

    fn from_str(s: &str) -> Result<DataType, Self::Err> {
        match s {
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
        let ns1 = TypeId::from_u128(0x11);
        let ns2 = TypeId::from_u128(0x22);

        // Same (namespace, name) → same id, every run.
        assert_eq!(
            TypeId::from_name(ns1, "Mode"),
            TypeId::from_name(ns1, "Mode")
        );
        // The name discriminates.
        assert_ne!(
            TypeId::from_name(ns1, "Mode"),
            TypeId::from_name(ns1, "Other")
        );
        // The namespace discriminates (same name, different namespace).
        assert_ne!(
            TypeId::from_name(ns1, "Mode"),
            TypeId::from_name(ns2, "Mode")
        );

        // It's a real UUIDv5 — the version nibble (hex digit 12) is 5 and the
        // upper bits are populated, unlike the old `u64`-widened hash.
        let id = TypeId::from_name(ns1, "Mode");
        assert_eq!((id.as_u128() >> 76) & 0xf, 5, "version nibble must be 5");
        assert_ne!(id.as_u128() >> 64, 0, "high 64 bits must carry entropy");
    }

    #[test]
    fn static_value_coercions() {
        // Numeric widening and bool→number.
        assert_eq!(StaticValue::Int(3).as_f64(), Some(3.0));
        assert_eq!(StaticValue::Bool(true).as_f64(), Some(1.0));
        assert_eq!(StaticValue::Bool(false).as_f64(), Some(0.0));
        assert_eq!(StaticValue::Float(2.7).as_i64(), Some(2)); // truncates
        assert_eq!(StaticValue::Int(9).as_i64(), Some(9));
        // Truthiness: 0.0 is false, anything past EPSILON is true.
        assert_eq!(StaticValue::Float(0.0).as_bool(), Some(false));
        assert_eq!(StaticValue::Float(0.5).as_bool(), Some(true));
        assert_eq!(StaticValue::Int(0).as_bool(), Some(false));
        // Non-numeric kinds don't coerce to numbers.
        assert_eq!(StaticValue::String("x".into()).as_f64(), None);
        assert_eq!(StaticValue::Null.as_i64(), None);
        // String / enum / path extractors.
        assert_eq!(StaticValue::String("hi".into()).as_string(), Some("hi"));
        assert_eq!(StaticValue::Int(1).as_string(), None);
        assert_eq!(StaticValue::Enum("Add".into()).as_enum(), Some("Add"));
        assert_eq!(
            StaticValue::FsPath("/tmp/x".into()).as_fs_path(),
            Some("/tmp/x")
        );
    }

    #[test]
    fn data_type_compatibility_is_wildcard_aware_and_otherwise_exact() {
        let custom = |id: u128| DataType::Custom(TypeId::from_u128(id));

        // Exact matches connect.
        assert!(DataType::Float.compatible_with(&DataType::Float));
        assert!(DataType::String.compatible_with(&DataType::String));

        // The scalar numerics coerce among themselves (matching the runtime), so
        // any pair of Float/Int/Bool is compatible…
        assert!(DataType::Float.compatible_with(&DataType::Int));
        assert!(DataType::Int.compatible_with(&DataType::Float));
        assert!(DataType::Bool.compatible_with(&DataType::Int));
        // …but a non-numeric never coerces to/from a numeric or another kind.
        assert!(!DataType::String.compatible_with(&DataType::Bool));
        assert!(!DataType::String.compatible_with(&DataType::Float));

        // `Null` is the wildcard on either side (the polymorphic passthrough port
        // and the default both read as `Null`).
        assert!(DataType::Null.compatible_with(&DataType::Float));
        assert!(DataType::Float.compatible_with(&DataType::Null));
        assert!(DataType::Null.compatible_with(&DataType::Null));

        // Custom types match by id, not by being "custom"; never to a scalar.
        assert!(custom(1).compatible_with(&custom(1)));
        assert!(!custom(1).compatible_with(&custom(2)));
        assert!(!custom(1).compatible_with(&DataType::Float));
    }

    #[test]
    fn dynamic_value_delegates_and_runtime_states() {
        // Static delegates to the inner StaticValue coercion.
        assert_eq!(
            DynamicValue::Static(StaticValue::Int(4)).as_f64(),
            Some(4.0)
        );
        assert_eq!(
            DynamicValue::Static(StaticValue::Int(4))
                .as_static()
                .map(StaticValue::as_i64),
            Some(Some(4))
        );
        // The two runtime-only states never coerce to a scalar.
        assert_eq!(DynamicValue::Unbound.as_f64(), None);
        assert!(DynamicValue::Unbound.as_static().is_none());
        // Default is Unbound.
        assert!(matches!(DynamicValue::default(), DynamicValue::Unbound));
    }

    #[test]
    fn conversions_wrap_in_static() {
        // Primitive `.into()` and `From<&StaticValue>` both land in `Static`.
        let d: DynamicValue = 3.5f64.into();
        assert!(matches!(d, DynamicValue::Static(StaticValue::Float(f)) if f == 3.5));
        let s = StaticValue::Int(9);
        assert_eq!(DynamicValue::from(&s).as_i64(), Some(9));
        assert_eq!(DynamicValue::from(true).as_bool(), Some(true));
    }

    #[test]
    fn datatype_default_value() {
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
            DataType::String.default_value(),
            Some(StaticValue::String(String::new()))
        );
        assert_eq!(
            DataType::FsPath(Arc::new(FsPathConfig::default())).default_value(),
            Some(StaticValue::FsPath(String::new()))
        );
        // Custom has no authorable literal; an enum's first-variant default needs
        // the library's variant list, so the bare type yields `None` here too.
        assert_eq!(DataType::Custom(TypeId::from_u128(1)).default_value(), None);
        assert_eq!(DataType::Enum(TypeId::from_u128(2)).default_value(), None);
    }

    #[test]
    fn display_formats() {
        assert_eq!(format!("{}", DynamicValue::Unbound), "-");
        assert_eq!(
            format!("{}", DynamicValue::Static(StaticValue::Int(5))),
            "5"
        );
        assert_eq!(format!("{}", StaticValue::String("hi".into())), "\"hi\"");
        assert_eq!(format!("{}", StaticValue::Float(1.5)), "1.5000");
        assert_eq!(format!("{}", StaticValue::Null), "null");
    }
}
