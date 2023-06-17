use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::data_type::*;

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
}

#[derive(Default, Serialize, Deserialize)]
pub struct Functions {
    functions: Vec<Function>,
}

impl Functions {
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