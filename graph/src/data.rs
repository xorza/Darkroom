use std::any::Any;
use std::fmt::Display;
use std::hash::{Hash, Hasher};
use std::str::FromStr;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use common::id_type;

id_type!(TypeId);

/// Trait for custom types that can be stored in `DynamicValue::Custom`.
/// Implementors provide their `DataType` so it doesn't need to be passed separately.
pub trait CustomValue: Any + Send + Sync {
    fn data_type(&self) -> DataType;
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
    pub fn index_of(&self, variant: &str) -> Option<usize> {
        self.variants.iter().position(|v| v == variant)
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

#[derive(Default, Debug, Clone)]
pub enum DynamicValue {
    #[default]
    None,
    Null,
    Float(f64),
    Int(i64),
    Bool(bool),
    String(String),
    // Array(Vec<DynamicValue>),
    Custom {
        type_id: TypeId,
        data: Arc<dyn Any + Send + Sync>,
    },
    Enum {
        type_id: TypeId,
        variant_name: String,
    },
}

impl StaticValue {
    pub fn as_float(&self) -> f64 {
        match self {
            StaticValue::Float(value) => *value,
            _ => {
                panic!("Value is not a float")
            }
        }
    }
    pub fn as_int(&self) -> i64 {
        match self {
            StaticValue::Int(value) => *value,
            _ => {
                panic!("Value is not an int")
            }
        }
    }
    pub fn as_bool(&self) -> bool {
        match self {
            StaticValue::Bool(value) => *value,
            _ => {
                panic!("Value is not a bool")
            }
        }
    }
    pub fn as_string(&self) -> &str {
        match self {
            StaticValue::String(value) => value,
            _ => {
                panic!("Value is not a string")
            }
        }
    }

    pub fn as_enum(&self) -> &str {
        match self {
            StaticValue::Enum { variant_name, .. } => variant_name,
            _ => panic!("Value is not an enum"),
        }
    }
}

impl DynamicValue {
    pub fn custom<T: CustomValue>(value: T) -> Self {
        let DataType::Custom(type_def) = &value.data_type() else {
            panic!("")
        };

        DynamicValue::Custom {
            type_id: type_def.type_id,
            data: Arc::new(value),
        }
    }

    pub fn as_f64(&self) -> f64 {
        match self {
            DynamicValue::Float(value) => *value,
            _ => {
                panic!("Value is not a float")
            }
        }
    }
    pub fn as_i64(&self) -> i64 {
        self.try_into().unwrap()
    }
    pub fn none_or_int(&self) -> Option<i64> {
        match self {
            DynamicValue::Int(value) => Some(*value),
            DynamicValue::None => None,
            _ => panic!("Value is not a bool"),
        }
    }
    pub fn as_bool(&self) -> bool {
        match self {
            DynamicValue::Bool(value) => *value,
            _ => {
                panic!("Value is not a bool")
            }
        }
    }
    pub fn as_string(&self) -> &str {
        match self {
            DynamicValue::String(value) => value,
            _ => {
                panic!("Value is not a string")
            }
        }
    }
    pub fn as_custom<T: Any>(&self) -> &T {
        match self {
            DynamicValue::Custom { data, .. } => data
                .downcast_ref::<T>()
                .expect("Custom value type mismatch"),
            _ => {
                panic!("Value is not a custom type")
            }
        }
    }

    pub fn as_enum(&self) -> &str {
        match self {
            DynamicValue::Enum { variant_name, .. } => variant_name,
            _ => panic!("Value is not an enum"),
        }
    }

    pub fn is_none(&self) -> bool {
        matches!(self, DynamicValue::None)
    }

    pub fn unwrap_or_f64(&self, default: f64) -> f64 {
        match self {
            DynamicValue::None => default,
            _ => self.as_f64(),
        }
    }

    pub fn unwrap_or_i64(&self, default: i64) -> i64 {
        match self {
            DynamicValue::None => default,
            _ => self.as_i64(),
        }
    }

    pub fn unwrap_or_bool(&self, default: bool) -> bool {
        match self {
            DynamicValue::None => default,
            _ => self.as_bool(),
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

impl From<StaticValue> for DynamicValue {
    fn from(value: StaticValue) -> Self {
        match value {
            StaticValue::Null => DynamicValue::Null,
            StaticValue::Float(value) => DynamicValue::Float(value),
            StaticValue::Int(value) => DynamicValue::Int(value),
            StaticValue::Bool(value) => DynamicValue::Bool(value),
            StaticValue::String(value) => DynamicValue::String(value),
            StaticValue::Enum {
                type_id,
                variant_name,
            } => DynamicValue::Enum {
                type_id,
                variant_name,
            },
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

impl From<DataType> for DynamicValue {
    fn from(data_type: DataType) -> Self {
        Self::from(&data_type)
    }
}

impl From<&DataType> for StaticValue {
    fn from(data_type: &DataType) -> Self {
        match data_type {
            DataType::Float => StaticValue::Float(0.0),
            DataType::Int => StaticValue::Int(0),
            DataType::Bool => StaticValue::Bool(false),
            DataType::String => StaticValue::String("".to_string()),
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
        match data_type {
            DataType::Float => DynamicValue::Float(0.0),
            DataType::Int => DynamicValue::Int(0),
            DataType::Bool => DynamicValue::Bool(false),
            DataType::String => DynamicValue::String("".to_string()),
            DataType::Enum(enum_def) => DynamicValue::Enum {
                type_id: enum_def.type_id,
                variant_name: enum_def.variants[0].clone(),
            },
            _ => panic!("No value for {:?}", data_type),
        }
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

impl TryFrom<&DynamicValue> for i64 {
    type Error = ();

    fn try_from(value: &DynamicValue) -> Result<Self, Self::Error> {
        match value {
            DynamicValue::Int(value) => Ok(*value),
            _ => Err(()),
        }
    }
}

impl DataType {
    pub fn is_custom(&self) -> bool {
        matches!(self, DataType::Custom(_))
    }
}

impl Display for DataType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let str = match &self {
            DataType::Float => "float".to_string(),
            DataType::Int => "int".to_string(),
            DataType::Bool => "bool".to_string(),
            DataType::String => "string".to_string(),
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

            DataType::Array { element_type, .. } => {
                5.hash(state);
                element_type.hash(state);
                // length is not included in the hash
            }

            DataType::Custom(def) => {
                6.hash(state);
                def.type_id.hash(state);
            }

            DataType::Enum(def) => {
                7.hash(state);
                def.type_id.hash(state);
                // type_name and variants are not included in the hash
            }
        }
    }
}
