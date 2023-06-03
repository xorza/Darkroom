use serde::{Deserialize, Serialize};

use crate::data_type::*;

#[derive(Default, Clone, Serialize, Deserialize)]
pub struct Arg {
    pub name: String,
    pub data_type: DataType,
}

#[derive(Default, Clone, Serialize, Deserialize)]
pub struct Function {
    self_id: u32,
    pub name: String,
    pub inputs: Vec<Arg>,
    pub outputs: Vec<Arg>,
}

#[derive(Default, Serialize, Deserialize)]
pub struct FunctionGraph {
    functions: Vec<Function>,
}

impl FunctionGraph {
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

    pub fn function_by_node_id(&self, node_id: u32) -> Option<&Function> {
        self.functions.iter().find(|func| func.self_id == node_id)
    }
    pub fn function_by_node_id_mut(&mut self, node_id: u32) -> Option<&mut Function> {
        self.functions.iter_mut().find(|func| func.self_id == node_id)
    }
}

impl Function {
    pub fn new(self_id: u32) -> Function {
        Function {
            self_id,
            ..Self::default()
        }
    }

    pub fn id(&self) -> u32 {
        self.self_id
    }
}