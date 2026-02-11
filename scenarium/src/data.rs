use std::any::Any;
use std::fmt::Display;
use std::hash::{Hash, Hasher};
use std::str::FromStr;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use strum::VariantNames;

use common::{SharedFn, id_type};

use crate::context::ContextManager;

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

pub type PreviewFn = SharedFn<
    dyn Fn(&dyn Any, &mut ContextManager) -> Option<Box<dyn PendingPreview>> + Send + Sync,
>;

/// Trait for pending preview generation that requires polling to complete.
#[async_trait::async_trait]
pub trait PendingPreview: Send {
    /// Awaits until the preview generation completes.
    async fn wait(self: Box<Self>, ctx_manager: &mut ContextManager);
}

/// Trait for custom types that can be stored in `DynamicValue::Custom`.
/// Implementors provide their `DataType` so it doesn't need to be passed separately.
pub trait CustomValue: Any + Send + Sync + Display {
    fn data_type(&self) -> DataType;
    fn gen_preview(&self, _ctx_manager: &mut ContextManager) -> Option<Box<dyn PendingPreview>> {
        None
    }
}

/// Definition of a custom type for `DataType::Custom`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TypeDef {
    pub type_id: TypeId,
    // display_name is not included in the hash or equality check
    pub display_name: String,
}

/// Definition of an enum type for `DataType::Enum`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EnumDef {
    pub type_id: TypeId,
    pub display_name: String,
    pub variants: Vec<String>,
}

impl EnumDef {
    pub fn from_enum<E: EnumVariants>(
        type_id: impl Into<TypeId>,
        display_name: impl Into<String>,
    ) -> Self {
        Self {
            type_id: type_id.into(),
            display_name: display_name.into(),
            variants: E::variant_names(),
        }
    }

    pub fn index_of(&self, variant: &str) -> Option<usize> {
        self.variants.iter().position(|v| v == variant)
    }
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FsPathMode {
    #[default]
    ExistingFile,
    NewFile,
    Directory,
}

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
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

impl PartialEq for FsPathConfig {
    fn eq(&self, other: &Self) -> bool {
        self.mode == other.mode
    }
}

impl Eq for FsPathConfig {}

impl Hash for FsPathConfig {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.mode.hash(state);
        self.extensions.hash(state);
    }
}

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub enum DataType {
    #[default]
    Null,
    Float,
    Int,
    Bool,
    String,
    FsPath(FsPathConfig),
    Array {
        element_type: Box<DataType>,
        // length is not included in the hash or equality check
        length: usize,
    },
    Custom(Arc<TypeDef>),
    Enum(Arc<EnumDef>),
}

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub enum StaticValue {
    #[default]
    Null,
    Float(f64),
    Int(i64),
    Bool(bool),
    String(String),
    FsPath(String),
    Enum {
        type_id: TypeId,
        variant_name: String,
    },
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
            (
                StaticValue::Enum {
                    type_id: type_id_left,
                    variant_name: name_left,
                },
                StaticValue::Enum {
                    type_id: type_id_right,
                    variant_name: name_right,
                },
            ) => type_id_left == type_id_right && name_left == name_right,
            _ => false,
        }
    }
}

impl Eq for StaticValue {}

type DisplayFn = SharedFn<dyn Fn(&dyn Any) -> String + Send + Sync>;

#[derive(Default, Clone)]
pub enum DynamicValue {
    #[default]
    None,
    Null,
    Float(f64),
    Int(i64),
    Bool(bool),
    String(String),
    FsPath(String),
    // Array(Vec<DynamicValue>),
    Custom {
        type_id: TypeId,
        data: Arc<dyn Any + Send + Sync>,
        display_fn: DisplayFn,
        preview_fn: PreviewFn,
    },
    Enum {
        type_id: TypeId,
        variant_name: String,
    },
}

impl std::fmt::Debug for DynamicValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DynamicValue::None => write!(f, "None"),
            DynamicValue::Null => write!(f, "Null"),
            DynamicValue::Float(v) => f.debug_tuple("Float").field(v).finish(),
            DynamicValue::Int(v) => f.debug_tuple("Int").field(v).finish(),
            DynamicValue::Bool(v) => f.debug_tuple("Bool").field(v).finish(),
            DynamicValue::String(v) => f.debug_tuple("String").field(v).finish(),
            DynamicValue::FsPath(v) => f.debug_tuple("FsPath").field(v).finish(),
            DynamicValue::Custom { type_id, .. } => f
                .debug_struct("Custom")
                .field("type_id", type_id)
                .finish_non_exhaustive(),
            DynamicValue::Enum {
                type_id,
                variant_name,
            } => f
                .debug_struct("Enum")
                .field("type_id", type_id)
                .field("variant_name", variant_name)
                .finish(),
        }
    }
}

impl DynamicValue {
    pub fn from_custom<T: CustomValue + 'static>(value: T) -> Self {
        let DataType::Custom(type_def) = &value.data_type() else {
            panic!("value expected to be of custom type");
        };

        let type_id = type_def.type_id;
        let display_fn: DisplayFn = SharedFn::new(Arc::new(|data: &dyn Any| {
            data.downcast_ref::<T>()
                .map(|v| v.to_string())
                .unwrap_or_else(|| "<invalid>".to_string())
        }));
        let preview_fn: PreviewFn = SharedFn::new(Arc::new(
            |data: &dyn Any, ctx_manager: &mut ContextManager| {
                data.downcast_ref::<T>()
                    .and_then(|custom_value| custom_value.gen_preview(ctx_manager))
            },
        ));

        DynamicValue::Custom {
            type_id,
            data: Arc::new(value),
            display_fn,
            preview_fn,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            DynamicValue::Float(value) => Some(*value),
            DynamicValue::Int(value) => Some(*value as f64),
            DynamicValue::Bool(value) => Some(*value as i64 as f64),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            DynamicValue::Int(value) => Some(*value),
            DynamicValue::Float(value) => Some(*value as i64),
            DynamicValue::Bool(value) => Some(*value as i64),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            DynamicValue::Bool(value) => Some(*value),
            DynamicValue::Int(value) => Some(*value != 0),
            DynamicValue::Float(value) => Some(value.abs() > f64::EPSILON),
            _ => None,
        }
    }

    pub fn as_string(&self) -> Option<&str> {
        match self {
            DynamicValue::String(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_custom<T: CustomValue>(&self) -> Option<&T> {
        match self {
            DynamicValue::Custom { data, .. } => data.downcast_ref::<T>(),
            _ => None,
        }
    }

    pub fn as_enum(&self) -> Option<&str> {
        match self {
            DynamicValue::Enum { variant_name, .. } => Some(variant_name),
            _ => None,
        }
    }

    pub fn as_fs_path(&self) -> Option<&str> {
        match self {
            DynamicValue::FsPath(value) => Some(value),
            _ => None,
        }
    }

    pub fn gen_preview(
        &mut self,
        ctx_manager: &mut ContextManager,
    ) -> Option<Box<dyn PendingPreview>> {
        if let DynamicValue::Custom {
            data, preview_fn, ..
        } = self
            && let Some(preview_fn) = preview_fn.as_ref()
        {
            (preview_fn)(data.as_ref(), ctx_manager)
        } else {
            None
        }
    }

    pub fn convert_type(self, dst_data_type: &DataType) -> DynamicValue {
        match (&self, dst_data_type) {
            (DynamicValue::None, _) => DynamicValue::None,

            (DynamicValue::Bool(_), DataType::Bool) => self,
            (DynamicValue::Bool(v), DataType::Int) => DynamicValue::Int(*v as i64),
            (DynamicValue::Bool(v), DataType::Float) => DynamicValue::Float(*v as i64 as f64),
            (DynamicValue::Bool(v), DataType::String) => DynamicValue::String(v.to_string()),

            (DynamicValue::Int(_), DataType::Int) => self,
            (DynamicValue::Int(v), DataType::Bool) => DynamicValue::Bool(*v != 0),
            (DynamicValue::Int(v), DataType::Float) => DynamicValue::Float(*v as f64),
            (DynamicValue::Int(v), DataType::String) => DynamicValue::String(v.to_string()),

            (DynamicValue::Float(_), DataType::Float) => self,
            (DynamicValue::Float(v), DataType::Bool) => DynamicValue::Bool(v.abs() > f64::EPSILON),
            (DynamicValue::Float(v), DataType::Int) => DynamicValue::Int(*v as i64),
            (DynamicValue::Float(v), DataType::String) => DynamicValue::String(v.to_string()),

            (DynamicValue::String(_), DataType::String) => self,
            (DynamicValue::String(s), DataType::Int) => DynamicValue::Int(s.parse().unwrap_or(0)),
            (DynamicValue::String(s), DataType::Float) => {
                DynamicValue::Float(s.parse().unwrap_or(0.0))
            }
            (DynamicValue::String(s), DataType::Bool) => {
                DynamicValue::Bool(s == "true" || s == "1")
            }
            (DynamicValue::String(s), DataType::FsPath(_)) => DynamicValue::FsPath(s.clone()),

            (DynamicValue::FsPath(_), DataType::FsPath(_)) => self,
            (DynamicValue::FsPath(s), DataType::String) => DynamicValue::String(s.clone()),

            (DynamicValue::Enum { type_id, .. }, DataType::Enum(enum_def))
                if *type_id == enum_def.type_id =>
            {
                self.clone()
            }

            (DynamicValue::Custom { type_id, .. }, DataType::Custom(type_def))
                if *type_id == type_def.type_id =>
            {
                self.clone()
            }

            (src, dst) => {
                panic!("Unsupported conversion from {:?} to {:?}", src, dst);
            }
        }
    }
}

impl From<&StaticValue> for DynamicValue {
    fn from(value: &StaticValue) -> Self {
        match value {
            StaticValue::Null => DynamicValue::Null,
            StaticValue::Float(value) => DynamicValue::Float(*value),
            StaticValue::Int(value) => DynamicValue::Int(*value),
            StaticValue::Bool(value) => DynamicValue::Bool(*value),
            StaticValue::String(value) => DynamicValue::String(value.clone()),
            StaticValue::FsPath(value) => DynamicValue::FsPath(value.clone()),
            StaticValue::Enum {
                type_id,
                variant_name,
            } => DynamicValue::Enum {
                type_id: *type_id,
                variant_name: variant_name.clone(),
            },
        }
    }
}

impl From<&DataType> for StaticValue {
    fn from(data_type: &DataType) -> Self {
        match data_type {
            DataType::Float => StaticValue::Float(0.0),
            DataType::Int => StaticValue::Int(0),
            DataType::Bool => StaticValue::Bool(false),
            DataType::String => StaticValue::String(String::new()),
            DataType::FsPath(_) => StaticValue::FsPath(String::new()),
            DataType::Enum(enum_def) => StaticValue::Enum {
                type_id: enum_def.type_id,
                variant_name: enum_def.variants[0].clone(),
            },
            _ => panic!("No value for {:?}", data_type),
        }
    }
}

impl From<&DataType> for DynamicValue {
    fn from(data_type: &DataType) -> Self {
        DynamicValue::from(&StaticValue::from(data_type))
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

impl From<i64> for DynamicValue {
    fn from(value: i64) -> Self {
        DynamicValue::Int(value)
    }
}

impl From<i32> for DynamicValue {
    fn from(value: i32) -> Self {
        DynamicValue::Int(value as i64)
    }
}

impl From<f32> for DynamicValue {
    fn from(value: f32) -> Self {
        DynamicValue::Float(value as f64)
    }
}

impl From<f64> for DynamicValue {
    fn from(value: f64) -> Self {
        DynamicValue::Float(value)
    }
}

impl From<&str> for DynamicValue {
    fn from(value: &str) -> Self {
        DynamicValue::String(value.to_string())
    }
}

impl From<String> for DynamicValue {
    fn from(value: String) -> Self {
        DynamicValue::String(value)
    }
}

impl From<bool> for DynamicValue {
    fn from(value: bool) -> Self {
        DynamicValue::Bool(value)
    }
}

impl DataType {
    pub fn from_custom(type_id: impl Into<TypeId>, display_name: impl Into<String>) -> Self {
        DataType::Custom(Arc::new(TypeDef {
            type_id: type_id.into(),
            display_name: display_name.into(),
        }))
    }

    pub fn from_enum<E: EnumVariants>(
        type_id: impl Into<TypeId>,
        display_name: impl Into<String>,
    ) -> Self {
        DataType::Enum(Arc::new(EnumDef::from_enum::<E>(type_id, display_name)))
    }

    pub fn default_value(&self) -> StaticValue {
        StaticValue::from(self)
    }

    pub fn is_custom(&self) -> bool {
        matches!(self, DataType::Custom(_))
    }
}

impl Display for DynamicValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DynamicValue::None => write!(f, "-"),
            DynamicValue::Null => write!(f, "null"),
            DynamicValue::Float(v) => write!(f, "{v:.4}"),
            DynamicValue::Int(v) => write!(f, "{v}"),
            DynamicValue::Bool(v) => write!(f, "{v}"),
            DynamicValue::String(s) => write!(f, "\"{s}\""),
            DynamicValue::FsPath(s) => write!(f, "\"{s}\""),
            DynamicValue::Custom {
                data, display_fn, ..
            } => {
                if let Some(display_fn) = display_fn.as_ref() {
                    write!(f, "{}", display_fn(data.as_ref()))
                } else {
                    write!(f, "<custom>")
                }
            }
            DynamicValue::Enum { variant_name, .. } => write!(f, "{variant_name}"),
        }
    }
}

impl Display for DataType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let str = match &self {
            DataType::Float => "float".to_string(),
            DataType::Int => "int".to_string(),
            DataType::Bool => "bool".to_string(),
            DataType::String => "string".to_string(),
            DataType::FsPath(_) => "path".to_string(),
            DataType::Custom(def) => def.display_name.clone(),
            _ => panic!("No string representation for {:?}", self),
        };
        write!(f, "{}", str)
    }
}

impl FromStr for DataType {
    type Err = ();

    fn from_str(s: &str) -> Result<DataType, Self::Err> {
        match s {
            "float" => Ok(DataType::Float),
            "number" => Ok(DataType::Float),
            "int" => Ok(DataType::Int),
            "bool" => Ok(DataType::Bool),
            "string" => Ok(DataType::String),
            "path" => Ok(DataType::FsPath(FsPathConfig::default())),
            _ => Err(()),
        }
    }
}

impl PartialEq for DataType {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (DataType::Null, DataType::Null) => true,
            (DataType::Float, DataType::Float) => true,
            (DataType::Int, DataType::Int) => true,
            (DataType::Bool, DataType::Bool) => true,
            (DataType::String, DataType::String) => true,
            (DataType::FsPath(config_a), DataType::FsPath(config_b)) => config_a == config_b,

            (DataType::Custom(def1), DataType::Custom(def2)) => def1.type_id == def2.type_id,

            (
                DataType::Array {
                    element_type: element_type_a,
                    ..
                },
                DataType::Array {
                    element_type: element_type_b,
                    ..
                },
            ) => element_type_a == element_type_b,

            (DataType::Enum(def_a), DataType::Enum(def_b)) => def_a.type_id == def_b.type_id,

            _ => false,
        }
    }
}

impl Eq for DataType {}

impl Hash for DataType {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            DataType::Null => 0.hash(state),
            DataType::Float => 1.hash(state),
            DataType::Int => 2.hash(state),
            DataType::Bool => 3.hash(state),
            DataType::String => 4.hash(state),
            DataType::FsPath(config) => {
                5.hash(state);
                config.hash(state);
            }

            DataType::Array { element_type, .. } => {
                6.hash(state);
                element_type.hash(state);
                // length is not included in the hash
            }

            DataType::Custom(def) => {
                7.hash(state);
                def.type_id.hash(state);
            }

            DataType::Enum(def) => {
                8.hash(state);
                def.type_id.hash(state);
                // type_name and variants are not included in the hash
            }
        }
    }
}
