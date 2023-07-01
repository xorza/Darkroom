use std::str::FromStr;

use serde::{Deserialize, Serialize};

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug, Serialize, Deserialize)]
pub enum DataType {
    #[default]
    None,
    Float,
    Int,
    Bool,
    String,
}

impl DataType {
    pub fn can_assign(from: &DataType, to: &DataType) -> bool {
        assert_ne!(from, &DataType::None);
        assert_ne!(to, &DataType::None);

        from == to
    }
}

impl ToString for DataType {
    fn to_string(&self) -> String {
        match self {
            DataType::Float => "float".to_string(),
            DataType::Int => "int".to_string(),
            DataType::Bool => "bool".to_string(),
            DataType::String => "string".to_string(),
            _ => panic!("No string representation for {:?}", self),
        }
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


#[repr(C)]
#[derive(Clone, PartialEq, Default, Debug, Serialize, Deserialize)]
pub enum Value {
    #[default]
    Null,
    Float(f64),
    Int(i64),
    Bool(bool),
    String(String),
}

impl Value {}


impl From<DataType> for Value {
    fn from(data_type: DataType) -> Self {
        match data_type {
            DataType::Float => Value::Float(0.0),
            DataType::Int => Value::Int(0),
            DataType::Bool => Value::Bool(false),
            DataType::String => Value::String("".to_string()),
            _ => panic!("No value for {:?}", data_type),
        }
    }
}

impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Value::Int(value)
    }
}

impl From<i32> for Value {
    fn from(value: i32) -> Self {
        Value::Int(value as i64)
    }
}

impl From<f32> for Value {
    fn from(value: f32) -> Self {
        Value::Float(value as f64)
    }
}

impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Value::Float(value)
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Value::String(value.to_string())
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Value::String(value)
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Value::Bool(value)
    }
}

