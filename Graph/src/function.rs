use serde::{Deserialize, Serialize};

use common::id_type;

use crate::data::*;
use crate::graph::FunctionBehavior;

#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub struct OutputInfo {
    pub name: String,
    pub data_type: DataType,
}


#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub struct InputInfo {
    pub name: String,
    pub is_required: bool,
    pub data_type: DataType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_value: Option<StaticValue>,
    #[serde(default, skip_serializing_if = "skip_serializing_if_none_or_empty")]
    pub variants: Option<Vec<(StaticValue, String)>>,
}

id_type!(FunctionId);

#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub struct Function {
    pub self_id: FunctionId,
    pub name: String,
    pub category: String,
    pub behavior: FunctionBehavior,
    pub is_output: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<InputInfo>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<OutputInfo>,
}


impl Function {
    pub fn new(func_id: FunctionId) -> Function {
        Function {
            self_id: func_id,
            ..Self::default()
        }
    }
}


fn skip_serializing_if_none_or_empty(opt: &Option<Vec<(StaticValue, String)>>) -> bool {
    opt.as_ref().map_or(true, |v| v.is_empty())
}

