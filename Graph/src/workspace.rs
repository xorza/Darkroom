use crate::function_graph::FunctionGraph;
use crate::graph::Graph;
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize)]
pub struct Workspace {
    function_graph: FunctionGraph,
    graph: Graph,
}

impl Workspace {
    pub fn new() -> Workspace {
        Workspace {
            function_graph: FunctionGraph::new(),
            graph: Graph::new(),
        }
    }

    pub fn graph_mut(&mut self) -> &mut Graph {
        &mut self.graph
    }
    pub fn graph(&self) -> &Graph {
        &self.graph
    }
    pub fn function_graph_mut(&mut self) -> &mut FunctionGraph {
        &mut self.function_graph
    }
    pub fn function_graph(&self) -> &FunctionGraph {
        &self.function_graph
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(&self).unwrap()
    }
    pub fn from_json_file(path: &str) -> Workspace {
        let json = std::fs::read_to_string(path).unwrap();
        let workspace: Workspace = serde_json::from_str(&json).unwrap();

        if !workspace.graph().validate() {
            panic!("Invalid workspace");
        }

        return workspace;
    }
}
