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
pub enum ConnectionBehavior {
    Always,
    Once,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Node {
    self_id: u32,

    pub name: String,
    pub behavior: NodeBehavior,
    pub is_output: bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Output {
    self_id: u32,
    node_id: u32,

    pub name: String,
    pub data_type: DataType,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Input {
    self_id: u32,
    node_id: u32,

    pub name: String,
    pub data_type: DataType,
    pub is_required: bool,
    pub connected_output_id: u32,
    pub connection_behavior: ConnectionBehavior,
}


#[derive(Serialize, Deserialize)]
pub struct Graph {
    new_id: u32,

    nodes: Vec<Node>,
    inputs: Vec<Input>,
    outputs: Vec<Output>,
}


impl Graph {
    pub fn new() -> Graph {
        Graph {
            new_id: 5000,
            nodes: Vec::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
        }
    }

    fn new_id(&mut self) -> u32 {
        let id = self.new_id;
        self.new_id += 1;
        return id;
    }

    pub fn nodes(&self) -> &Vec<Node> {
        &self.nodes
    }
    pub fn inputs(&self) -> &Vec<Input> {
        &self.inputs
    }
    pub fn outputs(&self) -> &Vec<Output> {
        &self.outputs
    }

    pub fn add_node(&mut self, node: &mut Node) {
        if node.id() != 0 {
            return;
        } else {
            node.self_id = self.new_id();
            self.nodes.push(node.clone());
        }
    }
    pub fn add_input(&mut self, input: &mut Input) {
        if input.id() != 0 {
            return;
        } else {
            input.self_id = self.new_id();
            self.inputs.push(input.clone());
        }
    }
    pub fn add_output(&mut self, output: &mut Output) {
        if output.id() != 0 {
            return;
        } else {
            output.self_id = self.new_id();
            self.outputs.push(output.clone());
        }
    }

    pub fn remove_node_by_id(&mut self, id: u32) {
        let output_ids = self.outputs_by_node_id(id)
            .map(|output| output.self_id)
            .collect::<Vec<u32>>();

        self.inputs.iter_mut()
            .filter(|_input| output_ids.contains(&_input.connected_output_id))
            .for_each(|_input| _input.connected_output_id = 0);

        self.inputs.retain(|input| input.node_id != id);
        self.outputs.retain(|output| output.node_id != id);
        self.nodes.retain(|node| node.self_id != id);
    }
    pub fn remove_input_by_id(&mut self, id: u32) {
        self.inputs.retain(|input| input.self_id != id);
    }
    pub fn remove_output_by_id(&mut self, id: u32) {
        self.inputs.iter_mut()
            .filter(|_input| _input.connected_output_id == id)
            .for_each(|_input| _input.connected_output_id = 0);
        self.outputs.retain(|output| output.self_id != id);
    }

    pub fn node_by_id(&self, id: u32) -> Option<&Node> {
        assert_ne!(id, 0);
        self.nodes.iter().find(|node| node.self_id == id)
    }
    pub fn node_by_id_mut(&mut self, id: u32) -> Option<&mut Node> {
        assert_ne!(id, 0);
        self.nodes.iter_mut().find(|node| node.self_id == id)
    }

    pub fn output_by_id(&self, id: u32) -> Option<&Output> {
        assert_ne!(id, 0);
        self.outputs.iter().find(|output| output.self_id == id)
    }
    pub fn output_by_id_mut(&mut self, id: u32) -> Option<&mut Output> {
        assert_ne!(id, 0);
        self.outputs.iter_mut().find(|output| output.self_id == id)
    }

    pub fn input_by_id(&self, id: u32) -> Option<&Input> {
        assert_ne!(id, 0);
        self.inputs.iter().find(|input| input.self_id == id)
    }
    pub fn input_by_id_mut(&mut self, id: u32) -> Option<&mut Input> {
        assert_ne!(id, 0);
        self.inputs.iter_mut().find(|input| input.self_id == id)
    }

    pub fn inputs_by_node_id(&self, node_id: u32) -> impl Iterator<Item=&Input> {
        assert_ne!(node_id, 0);
        self.inputs.iter().filter(move |input| input.node_id == node_id)
    }
    pub fn outputs_by_node_id(&self, node_id: u32) -> impl Iterator<Item=&Output> {
        assert_ne!(node_id, 0);
        self.outputs.iter().filter(move |output| output.node_id == node_id)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(&self).unwrap()
    }
    pub fn from_json_file(path: &str) -> Graph {
        let json = std::fs::read_to_string(path).unwrap();
        let graph: Graph = serde_json::from_str(&json).unwrap();

        if !graph.validate() {
            panic!("Invalid graph");
        }

        return graph;
    }

    pub fn validate(&self) -> bool {
        if self.nodes.iter().any(|node| node.self_id == 0) {
            return false;
        }

        if self.inputs.iter().any(|input|
            {
                input.self_id == 0
                    || self.node_by_id(input.node_id).is_none()
                    || (input.connected_output_id != 0 && self.output_by_id(input.connected_output_id).is_none())
            }) {
            return false;
        }
        if self.outputs.iter().any(|output|
            {
                output.self_id == 0
                    || self.node_by_id(output.node_id).is_none()
            }) {
            return false;
        }

        return true;
    }
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

    pub fn id(&self) -> u32 {
        self.self_id
    }
}

impl Input {
    pub fn new(node_id: u32) -> Input {
        assert_ne!(node_id, 0);

        Input {
            self_id: 0,
            node_id,
            connected_output_id: 0,
            connection_behavior: ConnectionBehavior::Always,
            name: String::new(),
            data_type: DataType::None,
            is_required: false,
        }
    }
    pub fn id(&self) -> u32 {
        self.self_id
    }

    pub fn node_id(&self) -> u32 {
        self.node_id
    }
}

impl Output {
    pub fn new(node_id: u32) -> Output {
        assert_ne!(node_id, 0);

        Output {
            self_id: 0,
            node_id,
            name: String::new(),
            data_type: DataType::None,
        }
    }
    pub fn id(&self) -> u32 {
        self.self_id
    }
    pub fn node_id(&self) -> u32 {
        self.node_id
    }
}
