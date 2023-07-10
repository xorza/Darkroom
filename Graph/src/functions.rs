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
    pub data_type: DataType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub const_value: Option<Value>,
}

id_type!(FunctionId);

#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub struct Function {
    self_id: FunctionId,
    pub name: String,
    pub behavior: FunctionBehavior,
    pub is_output: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<InputInfo>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<OutputInfo>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Functions {
    functions: Vec<Function>,
}

impl Functions {
    pub fn new(funcs: &[&Function]) -> Functions {
        let functions = funcs.iter()
            .cloned()
            .cloned()
            .collect::<Vec<Function>>();

        Functions {
            functions,
        }
    }

    pub fn functions(&self) -> &Vec<Function> {
        &self.functions
    }

    pub fn add_function(&mut self, function: Function) {
        if let Some(func) = self.functions.iter_mut().find(|_func| _func.self_id == function.self_id) {
            *func = function;
        } else {
            self.functions.push(function);
        }
    }

    pub fn function_by_id(&self, func_id: FunctionId) -> Option<&Function> {
        self.functions.iter().find(|func| func.self_id == func_id)
    }
    pub fn function_by_id_mut(&mut self, func_id: FunctionId) -> Option<&mut Function> {
        self.functions.iter_mut().find(|func| func.self_id == func_id)
    }

    pub fn to_yaml(&self) -> anyhow::Result<String> {
        let yaml = serde_yaml::to_string(&self)?;
        Ok(yaml)
    }
    pub fn load_yaml_file(&mut self, path: &str) -> anyhow::Result<()> {
        let yaml = std::fs::read_to_string(path)?;
        let functions: Functions = serde_yaml::from_str(&yaml)?;
        self.functions = functions.functions;

        Ok(())
    }
}

impl Function {
    pub fn new(func_id: FunctionId) -> Function {
        Function {
            self_id: func_id,
            ..Self::default()
        }
    }

    pub fn id(&self) -> FunctionId {
        self.self_id
    }
}
