use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize)]
pub struct FunctionNode {
    pub function_id: u32,
    pub node_id: u32,
}

#[derive(Serialize, Deserialize)]
pub struct Argument {
    pub input_output_id: u32,
    pub function_id: u32,
    pub argument_id: u32,
}

#[derive(Serialize, Deserialize)]
pub struct Function {
    pub self_id: u32,
    pub name: String,
    pub arguments: Vec<String>,
}


