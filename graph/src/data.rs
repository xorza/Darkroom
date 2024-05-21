use std::any::Any;
use std::fmt::Display;
use std::hash::{Hash, Hasher};
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use common::id_type;

id_type!(TypeId);

#[repr(C)]
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
        length: u64,
    },
    Custom {
        type_id: TypeId,
        // type_name is not included in the hash or equality check
        type_name: String,
    },
}

#[repr(C)]
#[derive(Clone, PartialEq, Default, Debug, Serialize, Deserialize)]
pub enum StaticValue {
    #[default]
    Null,
    Float(f64),
    Int(i64),
    Bool(bool),
    String(String),
}

#[repr(C)]
#[derive(Default, Debug)]
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
        data_type: DataType,
        data: Box<dyn Any>,
    },
}

impl StaticValue {
    pub fn data_type(&self) -> &DataType {
        match self {
            StaticValue::Null => &DataType::Null,
            StaticValue::Float(_) => &DataType::Float,
            StaticValue::Int(_) => &DataType::Int,
            StaticValue::Bool(_) => &DataType::Bool,
            StaticValue::String(_) => &DataType::String,
        }
    }

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
}

impl DynamicValue {
    pub fn data_type(&self) -> &DataType {
        match self {
            DynamicValue::Null => &DataType::Null,
            DynamicValue::Float(_) => &DataType::Float,
            DynamicValue::Int(_) => &DataType::Int,
            DynamicValue::Bool(_) => &DataType::Bool,
            DynamicValue::String(_) => &DataType::String,
            // DynamicValue::Array(_) => DataType::Array,
            DynamicValue::Custom { data_type, .. } => data_type,
            DynamicValue::None => panic!("Value is None"),
        }
    }

    pub fn as_float(&self) -> f64 {
        match self {
            DynamicValue::Float(value) => *value,
            _ => {
                panic!("Value is not a float")
            }
        }
    }
    pub fn as_int(&self) -> i64 {
        match self {
            DynamicValue::Int(value) => *value,
            _ => {
                panic!("Value is not an int")
            }
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
    pub fn as_custom(&self) -> &Box<dyn Any> {
        match self {
            DynamicValue::Custom { data, .. } => data,
            _ => {
                panic!("Value is not a custom type")
            }
        }
    }

    pub fn is_none(&self) -> bool {
        matches!(self, DynamicValue::None)
    }
}

impl Clone for DynamicValue {
    fn clone(&self) -> Self {
        let mut result = DynamicValue::Null;
        result.clone_from(self);
        result
    }

    fn clone_from(&mut self, source: &Self) {
        match source {
            DynamicValue::Null => {
                *self = DynamicValue::Null;
            }
            DynamicValue::Float(value) => {
                *self = DynamicValue::Float(*value);
            }
            DynamicValue::Int(value) => {
                *self = DynamicValue::Int(*value);
            }
            DynamicValue::Bool(value) => {
                *self = DynamicValue::Bool(*value);
            }
            DynamicValue::String(value) => {
                *self = DynamicValue::String(value.clone());
            }
            // DynamicValue::Array(value) => { *self = DynamicValue::Array(value.clone()); }
            DynamicValue::Custom {
                data_type,
                data: _data,
            } => {
                *self = DynamicValue::Custom {
                    data_type: data_type.clone(),
                    // data: *data.clone(),
                    data: Box::new(()),
                };
                unimplemented!("Clone custom type")
            }
            DynamicValue::None => {
                *self = DynamicValue::None;
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
        }
    }
}
impl From<DataType> for StaticValue {
    fn from(data_type: DataType) -> Self {
        match data_type {
            DataType::Float => StaticValue::Float(0.0),
            DataType::Int => StaticValue::Int(0),
            DataType::Bool => StaticValue::Bool(false),
            DataType::String => StaticValue::String("".to_string()),
            _ => panic!("No value for {:?}", data_type),
        }
    }
}
impl From<DataType> for DynamicValue {
    fn from(data_type: DataType) -> Self {
        match data_type {
            DataType::Float => DynamicValue::Float(0.0),
            DataType::Int => DynamicValue::Int(0),
            DataType::Bool => DynamicValue::Bool(false),
            DataType::String => DynamicValue::String("".to_string()),
            _ => panic!("No value for {:?}", data_type),
        }
    }
}
impl From<&DataType> for StaticValue {
    fn from(data_type: &DataType) -> Self {
        match data_type {
            DataType::Float => StaticValue::Float(0.0),
            DataType::Int => StaticValue::Int(0),
            DataType::Bool => StaticValue::Bool(false),
            DataType::String => StaticValue::String("".to_string()),
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
impl From<StaticValue> for i64 {
    fn from(value: StaticValue) -> Self {
        match value {
            StaticValue::Int(value) => value,
            _ => {
                panic!("Value is not an int")
            }
        }
    }
}
impl From<StaticValue> for f64 {
    fn from(value: StaticValue) -> Self {
        match value {
            StaticValue::Float(value) => value,
            _ => {
                panic!("Value is not a float")
            }
        }
    }
}
impl From<StaticValue> for i32 {
    fn from(value: StaticValue) -> Self {
        match value {
            StaticValue::Int(value) => value as i32,
            _ => {
                panic!("Value is not an int")
            }
        }
    }
}
impl From<StaticValue> for f32 {
    fn from(value: StaticValue) -> Self {
        match value {
            StaticValue::Float(value) => value as f32,
            _ => {
                panic!("Value is not a float")
            }
        }
    }
}
impl From<StaticValue> for bool {
    fn from(value: StaticValue) -> Self {
        match value {
            StaticValue::Bool(value) => value,
            _ => {
                panic!("Value is not a bool")
            }
        }
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
impl From<DynamicValue> for i64 {
    fn from(value: DynamicValue) -> Self {
        match value {
            DynamicValue::Int(value) => value,
            _ => {
                panic!("Value is not an int")
            }
        }
    }
}
impl From<DynamicValue> for f64 {
    fn from(value: DynamicValue) -> Self {
        match value {
            DynamicValue::Float(value) => value,
            _ => {
                panic!("Value is not a float")
            }
        }
    }
}
impl From<DynamicValue> for i32 {
    fn from(value: DynamicValue) -> Self {
        match value {
            DynamicValue::Int(value) => value as i32,
            _ => {
                panic!("Value is not an int")
            }
        }
    }
}
impl From<DynamicValue> for f32 {
    fn from(value: DynamicValue) -> Self {
        match value {
            DynamicValue::Float(value) => value as f32,
            _ => {
                panic!("Value is not a float")
            }
        }
    }
}
impl From<DynamicValue> for bool {
    fn from(value: DynamicValue) -> Self {
        match value {
            DynamicValue::Bool(value) => value,
            _ => {
                panic!("Value is not a bool")
            }
        }
    }
}
impl From<DynamicValue> for String {
    fn from(value: DynamicValue) -> Self {
        match value {
            DynamicValue::String(value) => value,
            _ => {
                panic!("Value is not a string")
            }
        }
    }
}

impl DataType {
    pub fn is_custom(&self) -> bool {
        matches!(self, DataType::Custom { .. })
    }
}

impl Display for DataType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let str = match &self {
            DataType::Float => "float".to_string(),
            DataType::Int => "int".to_string(),
            DataType::Bool => "bool".to_string(),
            DataType::String => "string".to_string(),
            DataType::Custom { type_name, .. } => type_name.clone(),
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

            (DataType::Custom { type_id: id1, .. }, DataType::Custom { type_id: id2, .. }) => {
                id1 == id2
            }

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

            DataType::Custom { type_id, .. } => {
                6.hash(state);
                type_id.hash(state);
                // type_name is not included in the hash
            }
        }
    }
}
