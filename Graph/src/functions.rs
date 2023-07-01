use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::data::*;

#[derive(Default, Clone, Serialize, Deserialize)]
pub struct Arg {
    pub name: String,
    pub data_type: DataType,
}

#[derive(Default, Clone, Serialize, Deserialize)]
pub struct Function {
    self_id: Uuid,
    pub name: String,
    pub inputs: Vec<Arg>,
    pub outputs: Vec<Arg>,
    pub is_output: bool,
}

#[derive(Default, Serialize, Deserialize)]
pub struct Functions {
    functions: Vec<Function>,
}

impl Functions {
    pub fn new(funcs: &Vec<&Function>) -> Functions {
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

    pub fn function_by_node_id(&self, func_id: Uuid) -> Option<&Function> {
        self.functions.iter().find(|func| func.self_id == func_id)
    }
    pub fn function_by_node_id_mut(&mut self, func_id: Uuid) -> Option<&mut Function> {
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
    pub fn new(func_id: Uuid) -> Function {
        Function {
            self_id: func_id,
            ..Self::default()
        }
    }

    pub fn id(&self) -> Uuid {
        self.self_id
    }
}
