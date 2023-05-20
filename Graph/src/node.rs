use serde::{Serialize, Deserialize};

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeBehavior {
    Active,
    Passive,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataType {
    None,
    Float,
    Int,
    Bool,
    String,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EdgeBehavior {
    Always,
    Once,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Node {
    pub self_id: u32,
    pub name: String,
    pub behavior: NodeBehavior,
    pub is_output: bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Input {
    pub self_id: u32,
    pub node_id: u32,
    pub name: String,
    pub data_type: DataType,
    pub is_required: bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Output {
    pub self_id: u32,
    pub node_id: u32,
    pub name: String,
    pub data_type: DataType,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Edge {
    pub input_id: u32,
    pub output_id: u32,
    pub behavior: EdgeBehavior,
}


impl Node {
    pub fn new() -> Node {
        Node {
            self_id: 0,
            name: String::new(),
            behavior: NodeBehavior::Active,
            is_output: false,
        }
    }
}

impl Input {
    pub fn new() -> Input {
        Input {
            self_id: 0,
            node_id: 0,
            name: String::new(),
            data_type: DataType::None,
            is_required: false,
        }
    }
}

impl Output {
    pub fn new() -> Output {
        Output {
            self_id: 0,
            node_id: 0,
            name: String::new(),
            data_type: DataType::None,
        }
    }
}

impl Edge {
    pub fn new() -> Edge {
        Edge {
            input_id: 0,
            output_id: 0,
            behavior: EdgeBehavior::Always,
        }
    }
}
