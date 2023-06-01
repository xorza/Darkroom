use std::str::FromStr;
use serde::{Serialize, Deserialize};

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
pub enum DataType {
    None,
    Float,
    Int,
    Bool,
    String,
}

impl FromStr for DataType {
    type Err = ();

    fn from_str(s: &str) -> Result<DataType, Self::Err> {
        match s {
            "f32" => Ok(DataType::Float),
            "f64" => Ok(DataType::Float),
            "i32" => Ok(DataType::Int),
            "i64" => Ok(DataType::Int),
            "bool" => Ok(DataType::Bool),
            "string" => Ok(DataType::String),
            _ => Err(()),
        }
    }
}
