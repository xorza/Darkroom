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

/// A resident memory footprint split by where the bytes live: `cpu` system RAM
/// vs `gpu` VRAM. Per-value footprints ([`CustomValue::ram_bytes`]) fold into a
/// cache-wide total ([`RuntimeCache::resident_ram_usage`](crate::execution::cache::RuntimeCache::resident_ram_usage))
/// via [`Add`](std::ops::Add), which the editor surfaces as a RAM readout.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RamUsage {
    pub cpu: usize,
    pub gpu: usize,
}

impl RamUsage {
    /// Total bytes across both pools.
    pub fn total(&self) -> usize {
        self.cpu + self.gpu
    }
}

impl std::ops::Add for RamUsage {
    type Output = RamUsage;

    fn add(self, rhs: RamUsage) -> RamUsage {
        RamUsage {
            cpu: self.cpu + rhs.cpu,
            gpu: self.gpu + rhs.gpu,
        }
    }
}

impl std::ops::AddAssign for RamUsage {
    fn add_assign(&mut self, rhs: RamUsage) {
        self.cpu += rhs.cpu;
        self.gpu += rhs.gpu;
    }
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

    /// Owning counterpart of [`as_any`](Self::as_any), backing
    /// [`DynamicValue::into_custom`]. Always `{ self }` — it can't be defaulted
    /// because the unsize coercion needs `Self: Sized`, which would make the
    /// method uncallable on `dyn CustomValue`.
    fn into_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync>;

    /// This value's resident RAM footprint, split into system RAM vs GPU VRAM.
    /// Defaults to zero; heavy payloads (image buffers, stacked frames) override
    /// it so the runtime cache can report how much memory its resident values
    /// hold.
    fn ram_bytes(&self) -> RamUsage {
        RamUsage::default()
    }
}

/// Error a codec hands back to the framework. The codec lives in a downstream
/// crate, so its concrete failure stays type-erased here.
pub(crate) type CodecError = Box<dyn std::error::Error + Send + Sync>;

/// Bidirectional disk codec for one custom-value type, registered once on a type
/// entry in the [`Library`](crate::library::Library) type table — the disk-cache
/// counterpart of a [`CustomValue`]. Encode takes `&dyn CustomValue` (downcast to the
/// codec's concrete type) and is async + context-aware like [`CustomValue::gen_preview`],
/// so a GPU-resident value can read back through the [`ContextManager`]. Decode has only
/// bytes — there is no value on reload, which is why dispatch goes through the library
/// rather than a method on the value. The blob framing that calls this lives in
/// [`execution::codec`](crate::execution::codec).
#[async_trait::async_trait]
pub trait CustomValueCodec: Send + Sync + std::fmt::Debug {
    /// Encode `value` (always this codec's concrete type) for the cache, or `Err` if
    /// encoding failed (e.g. a GPU readback error) — surfaced to the caller rather than
    /// silently dropped. Whether a *type* is cacheable at all is decided by whether a
    /// codec is registered for it, not here.
    async fn encode(
        &self,
        value: &dyn CustomValue,
        ctx: &mut ContextManager,
    ) -> std::result::Result<Vec<u8>, CodecError>;

    /// Rebuild a value from bytes a prior [`Self::encode`] produced. Errors are expected
    /// when a blob outlives the binary that wrote it (corrupt or layout-changed bytes).
    ///
    /// **Contract:** a codec must *reject bytes it didn't write* — version its own frame
    /// and return `Err` on a mismatch. The framework treats a decode error as a cache miss
    /// and recomputes, but it can't tell a layout-changed blob from a valid one, so an
    /// unversioned codec that changes its byte format for the same `TypeId` (without the
    /// framework's `FORMAT_VERSION` or digest `DOMAIN` being bumped) would decode stale
    /// bytes into a wrong value — a silent false hit.
    fn decode(&self, bytes: Vec<u8>) -> std::result::Result<Arc<dyn CustomValue>, CodecError>;
}

/// Referent-identity resolver for a **resource-reference** type — one whose values *name*
/// external state (an asset id, a handle, a path-like key) rather than contain it.
/// Registered once on the type's [`Library`](crate::library::Library) entry, like a
/// [`CustomValueCodec`]. An input *declared* with such a type is the contract "this node
/// dereferences the reference": the digest folds the referent's current identity on top of
/// the structural fold, so a changed referent re-keys the consumer even when the reference
/// value (and everything upstream) is unchanged. `DataType::FsPath` is the built-in case
/// (file `(len, mtime)` / directory fingerprint) and needs no registration.
pub trait ResourceStamper: Send + Sync + std::fmt::Debug {
    /// Append the referent's *current* identity for `value` to `out` — cheap external-state
    /// metadata (a stat, a version counter, a cached etag), never a content read.
    /// Deterministic for a given (value, external state); called at digest time, up to once
    /// per run per consumer, so it must be fast. The digest folds `out` length-prefixed as
    /// one unit, so the encoding within is the stamper's own business.
    fn stamp(&self, value: &DynamicValue, out: &mut Vec<u8>);
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

/// A port's type. `Any` is the wildcard — a polymorphic passthrough / reroute
/// port that matches every type on either side (see [`Self::compatible_with`]);
/// it's also the declared type of an unresolved wildcard output and the default.
/// `Custom`/`Enum` reference a *registered* nominal type by [`TypeId`] — the
/// metadata (display name, enum variants, disk codec) lives on the
/// [`Library`](crate::library::Library) type table, not inline. `FsPath` is
/// structural config (not a nominal type), so it stays inline.
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

    /// The value's bare textual content — unquoted strings, full-precision
    /// floats, `true`/`false` for bools — as produced by the `To String` node.
    /// Distinct from [`Display`](StaticValue#impl-Display-for-StaticValue),
    /// which quotes strings and rounds floats for compact editor labels.
    pub fn to_value_string(&self) -> String {
        match self {
            StaticValue::Null => "null".to_string(),
            StaticValue::Float(value) => value.to_string(),
            StaticValue::Int(value) => value.to_string(),
            StaticValue::Bool(value) => value.to_string(),
            StaticValue::String(value) => value.clone(),
            StaticValue::FsPath(path) => path.clone(),
            StaticValue::Enum(variant) => variant.clone(),
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

    /// The custom value moved out of its `Arc` — the owning counterpart of
    /// [`as_custom`](Self::as_custom), for consumers that want to reuse the
    /// allocation in place. Succeeds only when this is a uniquely-held `Custom`
    /// of type `T`; otherwise returns `self` unchanged so the caller can fall
    /// back to cloning through the borrow. Must stay opportunistic: another
    /// holder (a RAM-cached slot, a second consumer, an in-flight inspection)
    /// legitimately keeps the `Arc` shared.
    pub fn into_custom<T: CustomValue>(self) -> Result<T, Self> {
        let DynamicValue::Custom(data) = self else {
            return Err(self);
        };
        if data.as_any().downcast_ref::<T>().is_none() {
            return Err(DynamicValue::Custom(data));
        }
        let typed = data
            .into_any()
            .downcast::<T>()
            .expect("type checked just above");
        Arc::try_unwrap(typed).map_err(|shared| DynamicValue::Custom(shared))
    }

    /// This value as text for the `To String` node — the inner value's
    /// [`StaticValue::to_value_string`], a custom value's `Display`, or the
    /// empty string when unbound.
    pub fn to_value_string(&self) -> String {
        match self {
            DynamicValue::Unbound => String::new(),
            DynamicValue::Static(value) => value.to_value_string(),
            DynamicValue::Custom(data) => data.to_string(),
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

    /// This value's resident RAM footprint: a [`Custom`](DynamicValue::Custom)
    /// payload's [`ram_bytes`](CustomValue::ram_bytes), else zero (inline scalars
    /// are negligible).
    pub fn ram_usage(&self) -> RamUsage {
        match self {
            DynamicValue::Custom(data) => data.ram_bytes(),
            _ => RamUsage::default(),
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
            DataType::Any => StaticValue::Null,
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
    /// - `Any` is the wildcard — a polymorphic passthrough / reroute port (see
    ///   [`OutputType::Wildcard`](crate::node::function::OutputType)) — and matches
    ///   anything on either side.
    /// - the scalar numerics (`Float`/`Int`/`Bool`) coerce among themselves at
    ///   runtime (`DynamicValue::as_f64`/`as_i64`/`as_bool` all accept the three),
    ///   so they're mutually compatible.
    /// - everything else must match exactly (derived `PartialEq`: `Custom`/`Enum`
    ///   by id, `FsPath` by config).
    pub fn compatible_with(&self, source: &DataType) -> bool {
        if matches!(self, DataType::Any) || matches!(source, DataType::Any) {
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

    /// A tiny displayable custom value shared by the tests below.
    #[derive(Debug)]
    struct Tag(&'static str);
    impl Display for Tag {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "tag:{}", self.0)
        }
    }
    impl CustomValue for Tag {
        fn type_id(&self) -> TypeId {
            TypeId::from_u128(0xaa)
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn into_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {
            self
        }
    }

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

        // `Any` is the wildcard on either side (the polymorphic passthrough port
        // and the default both read as `Any`).
        assert!(DataType::Any.compatible_with(&DataType::Float));
        assert!(DataType::Float.compatible_with(&DataType::Any));
        assert!(DataType::Any.compatible_with(&DataType::Any));

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
    fn to_value_string_is_bare_content_unlike_display() {
        // Every static kind renders as its bare content: no quotes on strings,
        // full-precision floats, canonical bool text.
        assert_eq!(StaticValue::Null.to_value_string(), "null");
        assert_eq!(StaticValue::Float(1.5).to_value_string(), "1.5");
        assert_eq!(StaticValue::Int(-42).to_value_string(), "-42");
        assert_eq!(StaticValue::Bool(true).to_value_string(), "true");
        assert_eq!(StaticValue::String("hi".into()).to_value_string(), "hi");
        assert_eq!(
            StaticValue::FsPath("/tmp/x".into()).to_value_string(),
            "/tmp/x"
        );
        assert_eq!(StaticValue::Enum("Add".into()).to_value_string(), "Add");

        // It deliberately diverges from `Display`, which quotes strings and
        // rounds floats to 4 places for compact editor labels.
        assert_eq!(format!("{}", StaticValue::String("hi".into())), "\"hi\"");
        assert_eq!(format!("{}", StaticValue::Float(1.5)), "1.5000");

        // DynamicValue delegates to the inner static value; unbound is empty; a
        // custom value renders through its `Display`.
        assert_eq!(
            DynamicValue::Static(StaticValue::Int(7)).to_value_string(),
            "7"
        );
        assert_eq!(DynamicValue::Unbound.to_value_string(), "");

        assert_eq!(
            DynamicValue::from_custom(Tag("z")).to_value_string(),
            "tag:z"
        );
    }

    #[test]
    fn into_custom_moves_unique_values_and_rejects_shared_or_foreign() {
        // Unique holder: the value moves out of its Arc.
        let unique = DynamicValue::from_custom(Tag("solo"));
        assert_eq!(unique.into_custom::<Tag>().unwrap().0, "solo");

        // A second Arc holder (a cached slot, another consumer) blocks the
        // move; the value comes back unchanged and stays readable by borrow.
        let first = DynamicValue::from_custom(Tag("shared"));
        let second = first.clone();
        let returned = first.into_custom::<Tag>().unwrap_err();
        assert_eq!(returned.as_custom::<Tag>().unwrap().0, "shared");
        // Once the other holder drops, the same value moves.
        drop(second);
        assert_eq!(returned.into_custom::<Tag>().unwrap().0, "shared");

        // A wrong target type hands the value back untouched.
        #[derive(Debug)]
        struct Other;
        impl Display for Other {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "other")
            }
        }
        impl CustomValue for Other {
            fn type_id(&self) -> TypeId {
                TypeId::from_u128(0xbb)
            }
            fn as_any(&self) -> &dyn Any {
                self
            }
            fn into_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {
                self
            }
        }
        let typed = DynamicValue::from_custom(Tag("typed"));
        let back = typed.into_custom::<Other>().unwrap_err();
        assert_eq!(back.as_custom::<Tag>().unwrap().0, "typed");

        // Non-custom values are handed back too.
        let unbound = DynamicValue::Unbound.into_custom::<Tag>().unwrap_err();
        assert!(matches!(unbound, DynamicValue::Unbound));
        let stat = DynamicValue::Static(StaticValue::Int(1))
            .into_custom::<Tag>()
            .unwrap_err();
        assert_eq!(stat.as_i64(), Some(1));
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
