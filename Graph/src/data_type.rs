use std::str::FromStr;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug, Serialize, Deserialize)]
#[repr(C)]
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
