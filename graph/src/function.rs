use std::str::FromStr;
use serde::{Deserialize, Serialize};

use common::id_type;

use crate::data::*;
use crate::graph::{ FunctionBehavior};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OutputInfo {
    pub name: String,
    pub data_type: DataType,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InputInfo {
    pub name: String,
    pub is_required: bool,
    pub data_type: DataType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_value: Option<StaticValue>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variants: Vec<(String, StaticValue)>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EventInfo {
    pub name: String,
}

id_type!(FunctionId);

#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub struct Function {
    pub id: FunctionId,
    pub name: String,
    pub category: String,
    pub behavior: FunctionBehavior,
    pub is_output: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<InputInfo>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<OutputInfo>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<EventInfo>,
}

impl Function {
    pub fn new(id: FunctionId) -> Function {
        Function {
            id,
            ..Self::default()
        }
    }
}

impl From<&str> for EventInfo {
    fn from(s: &str) -> Self {
        EventInfo { name: s.to_string() }
    }
}
impl FromStr for EventInfo {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(EventInfo { name: s.to_string() })
    }
}