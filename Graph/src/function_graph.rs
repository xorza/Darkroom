use serde::{Serialize, Deserialize};
use bevy_ecs::prelude::Component;
use crate::data_type::*;

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
pub enum Direction {
    In,
    Out,
}

#[derive(Clone, Serialize, Deserialize, Component)]
pub struct Arg {
    input_output_id: u32,
    node_id: u32,

    pub index: u32,
    pub direction: Direction,
    pub name: String,
    pub data_type: DataType,
}

#[derive(Clone, Serialize, Deserialize, Component)]
pub struct Function {
    node_id: u32,
    pub name: String,
}


#[derive(Serialize, Deserialize)]
pub struct FunctionGraph {
    functions: Vec<Function>,
    arguments: Vec<Arg>,
}

impl FunctionGraph {
    pub fn new() -> FunctionGraph {
        FunctionGraph {
            functions: Vec::new(),
            arguments: Vec::new(),
        }
    }

    pub fn functions(&self) -> &Vec<Function> {
        &self.functions
    }
    pub fn arguments(&self) -> &Vec<Arg> {
        &self.arguments
    }

    pub fn add_function(&mut self, function: Function) {
        if let Some(func) = self.functions.iter_mut().find(|_func| _func.node_id == function.node_id) {
            *func = function;
        } else {
            self.functions.push(function);
        }
    }
    pub fn add_argument(&mut self, argument: Arg) {
        if let Some(arg) = self.arguments.iter_mut().find(|arg| arg.input_output_id == argument.input_output_id) {
            *arg = argument;
        } else {
            self.arguments.push(argument);
        }
    }

    pub fn arg_by_input_output_id(&self, input_output_id: u32) -> Option<&Arg> {
        self.arguments.iter().find(|arg| arg.input_output_id == input_output_id)
    }

    pub fn function_by_node_id(&self, node_id: u32) -> Option<&Function> {
        self.functions.iter().find(|func| func.node_id == node_id)
    }
    pub fn function_by_node_id_mut(&mut self, node_id: u32) -> Option<&mut Function> {
        self.functions.iter_mut().find(|func| func.node_id == node_id)
    }

    pub fn args_by_node_id(&self, node_id: u32) -> impl Iterator<Item=&Arg> {
        self.arguments.iter().filter(move |arg| arg.node_id == node_id)
    }
}


impl Arg {
    pub fn new(input_output_id: u32, node_id: u32) -> Arg {
        Arg {
            input_output_id,
            node_id,
            index: 0,
            direction: Direction::In,
            name: String::new(),
            data_type: DataType::None,
        }
    }

    pub fn input_output_id(&self) -> u32 {
        self.input_output_id
    }
    pub fn node_id(&self) -> u32 {
        self.node_id
    }
}

impl Function {
    pub fn new(node_id: u32) -> Function {
        Function {
            node_id,
            name: String::new(),
        }
    }

    pub fn node_id(&self) -> u32 {
        self.node_id
    }
}