use serde::{Serialize, Deserialize};
use crate::graph::DataType;

#[derive(Serialize, Deserialize)]
pub enum Direction {
    In,
    Out,
}

#[derive(Serialize, Deserialize)]
pub struct Argument {
    input_output_id: u32,
    function_id: u32,
    pub direction: Direction,
    pub name: String,
    pub data_type: DataType,
}

#[derive(Serialize, Deserialize)]
pub struct Function {
    self_id: u32,
    pub name: String,
}


#[derive(Serialize, Deserialize)]
pub struct FunctionGraph {
    new_id: u32,

    functions: Vec<Function>,
    arguments: Vec<Argument>,
}

impl FunctionGraph {
    pub fn new() -> FunctionGraph {
        FunctionGraph {
            new_id: 15000,
            functions: Vec::new(),
            arguments: Vec::new(),
        }
    }

    pub fn functions(&self) -> &Vec<Function> {
        &self.functions
    }
    pub fn arguments(&self) -> &Vec<Argument> {
        &self.arguments
    }

    pub fn add_function(&mut self, mut function: Function) {
        if let Some(func) = self.functions.iter_mut().find(|func| func.self_id == function.self_id) {
            *func = function;
        } else {
            function.self_id = self.new_id;
            self.new_id += 1;
            self.functions.push(function);
        }
    }
}


impl Argument {
    pub fn new(input_output_id: u32, function_id: u32) -> Argument {
        Argument {
            input_output_id,
            function_id,
            direction: Direction::In,
            name: String::new(),
            data_type: DataType::None,
        }
    }
}

impl Function {
    pub fn new(id: u32) -> Function {
        Function {
            self_id: id,
            name: String::new(),
        }
    }
}