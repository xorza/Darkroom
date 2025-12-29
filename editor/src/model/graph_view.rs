use anyhow::{Result, anyhow, bail};
use common::{FileFormat, is_debug};
use graph::prelude::{Binding, Event, FuncLib, Graph as CoreGraph, Node, NodeId};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use uuid::Uuid;

use super::{Connection, Input, NodeView, Output};

#[derive(Debug, Serialize, Deserialize)]
pub struct GraphView {
    pub id: Uuid,
    pub nodes: Vec<NodeView>,
    pub pan: egui::Vec2,
    pub zoom: f32,
    pub selected_node_id: Option<Uuid>,
}

impl Default for GraphView {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            nodes: Vec::new(),
            pan: egui::Vec2::ZERO,
            zoom: 1.0,
            selected_node_id: None,
        }
    }
}

impl GraphView {
    pub fn from_graph(graph: &CoreGraph, func_lib: &FuncLib) -> Self {
        let mut nodes = Vec::with_capacity(graph.nodes.len());
        for (index, node) in graph.nodes.iter().enumerate() {
            let func = func_lib.by_id(node.func_id).unwrap_or_else(|| {
                panic!("Missing func for node {} ({})", node.name, node.func_id)
            });
            assert!(
                node.inputs.len() == func.inputs.len(),
                "node inputs must match function inputs"
            );

            let mut inputs = Vec::with_capacity(node.inputs.len());
            for (input_index, input) in node.inputs.iter().enumerate() {
                let func_input = func
                    .inputs
                    .get(input_index)
                    .expect("func inputs must align with node inputs");
                let connection = match &input.binding {
                    Binding::Output(binding) => Some(Connection {
                        node_id: binding.output_node_id.as_uuid(),
                        output_index: binding.output_idx,
                    }),
                    Binding::None | Binding::Const => None,
                };
                inputs.push(Input {
                    name: func_input.name.clone(),
                    connection,
                });
            }

            let mut outputs = Vec::with_capacity(func.outputs.len());
            for output in &func.outputs {
                outputs.push(Output {
                    name: output.name.clone(),
                });
            }

            let column = index % 3;
            let row = index / 3;
            let pos = egui::pos2(80.0 + 240.0 * column as f32, 120.0 + 180.0 * row as f32);

            nodes.push(NodeView {
                id: node.id.as_uuid(),
                name: node.name.clone(),
                pos,
                inputs,
                outputs,
                behavior: node.behavior,
                terminal: node.terminal,
            });
        }

        let graph = Self {
            id: Uuid::new_v4(),
            nodes,
            pan: egui::Vec2::ZERO,
            zoom: 1.0,
            selected_node_id: None,
        };
        graph
            .validate()
            .expect("graph view should be valid after conversion");
        graph
    }

    pub fn to_graph(&self, func_lib: &FuncLib) -> CoreGraph {
        let mut graph = CoreGraph::default();
        let mut node_ids = HashMap::new();
        let mut output_counts = HashMap::new();
        for node in &self.nodes {
            let prior = node_ids.insert(node.id, node.name.as_str());
            assert!(prior.is_none(), "graph view node ids must be unique");
            let prior = output_counts.insert(node.id, node.outputs.len());
            assert!(prior.is_none(), "graph view node ids must be unique");
        }

        for node_view in &self.nodes {
            let func = func_lib.by_name(&node_view.name).unwrap_or_else(|| {
                panic!(
                    "Missing func named {} for node {}",
                    node_view.name, node_view.id
                )
            });
            let func_id = func.id;
            assert!(
                node_view.inputs.len() == func.inputs.len(),
                "node inputs must match function inputs"
            );
            assert!(
                node_view.outputs.len() == func.outputs.len(),
                "node outputs must match function outputs"
            );

            let mut inputs = Vec::with_capacity(func.inputs.len());
            for (input_index, func_input) in func.inputs.iter().enumerate() {
                let view_input = node_view
                    .inputs
                    .get(input_index)
                    .expect("graph view inputs must align with function inputs");
                let (binding, const_value) = match &view_input.connection {
                    Some(connection) => {
                        assert!(
                            node_ids.contains_key(&connection.node_id),
                            "connection must reference an existing node"
                        );
                        let output_count = output_counts
                            .get(&connection.node_id)
                            .copied()
                            .expect("connection must reference an existing node");
                        assert!(
                            connection.output_index < output_count,
                            "connection output index must be in range"
                        );
                        (
                            Binding::from_output_binding(
                                NodeId::from(connection.node_id),
                                connection.output_index,
                            ),
                            None,
                        )
                    }
                    None => func_input
                        .default_value
                        .as_ref()
                        .map_or((Binding::None, None), |_| {
                            (Binding::Const, func_input.default_value.clone())
                        }),
                };
                inputs.push(graph::graph::Input {
                    binding,
                    const_value,
                });
            }

            let events = (0..func.events.len()).map(|_| Event::default()).collect();
            let node = Node {
                id: NodeId::from(node_view.id),
                func_id,
                name: node_view.name.clone(),
                behavior: node_view.behavior,
                terminal: node_view.terminal,
                inputs,
                events,
            };
            graph.add(node);
        }

        graph
            .validate()
            .expect("graph must be valid after conversion");
        graph
    }

    pub fn validate(&self) -> Result<()> {
        if !is_debug() {
            return Ok(());
        }

        if !self.zoom.is_finite() || self.zoom <= 0.0 {
            return Err(anyhow!("graph zoom must be finite and positive"));
        }
        if !self.pan.x.is_finite() || !self.pan.y.is_finite() {
            return Err(anyhow!("graph pan must be finite"));
        }

        let mut output_counts = HashMap::new();
        for node in &self.nodes {
            if !node.pos.x.is_finite() || !node.pos.y.is_finite() {
                return Err(anyhow!("node position must be finite"));
            }
            let prior = output_counts.insert(node.id, node.outputs.len());
            if prior.is_some() {
                return Err(anyhow!("duplicate node id detected"));
            }
        }

        if let Some(selected_node_id) = self.selected_node_id
            && !output_counts.contains_key(&selected_node_id)
        {
            return Err(anyhow!("selected node id must exist in graph"));
        }

        for node in &self.nodes {
            for input in &node.inputs {
                if let Some(connection) = &input.connection {
                    let output_count = output_counts
                        .get(&connection.node_id)
                        .ok_or_else(|| anyhow!("connection references a missing node"))?;
                    if connection.output_index >= *output_count {
                        return Err(anyhow!("connection output index out of range"));
                    }
                }
            }
        }

        Ok(())
    }

    pub fn serialize(&self, format: FileFormat) -> String {
        self.validate()
            .expect("graph view must be valid before serialization");
        common::serialize(self, format)
    }

    pub fn deserialize(format: FileFormat, input: &str) -> Result<Self> {
        if input.trim().is_empty() {
            bail!("graph input is empty");
        }

        let graph = common::deserialize::<GraphView>(input, format)?;
        graph.validate()?;

        Ok(graph)
    }

    pub fn serialize_to_file<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let path = path.as_ref();
        let format = FileFormat::from_file_name(path.to_string_lossy().as_ref())
            .map_err(anyhow::Error::from)?;
        let payload = self.serialize(format);
        std::fs::write(path, payload).map_err(anyhow::Error::from)
    }

    pub fn deserialize_from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        let format = FileFormat::from_file_name(path.to_string_lossy().as_ref())
            .map_err(anyhow::Error::from)?;
        let payload = std::fs::read_to_string(path).map_err(anyhow::Error::from)?;

        Self::deserialize(format, &payload)
    }

    pub fn select_node(&mut self, node_id: Uuid) {
        assert!(
            self.nodes.iter().any(|node| node.id == node_id),
            "selected node must exist in graph"
        );
        let node_index = self
            .nodes
            .iter()
            .position(|node| node.id == node_id)
            .expect("selected node must exist in graph");
        if node_index + 1 != self.nodes.len() {
            let node = self.nodes.remove(node_index);
            self.nodes.push(node);
        }
        self.selected_node_id = Some(node_id);
    }

    pub fn remove_node(&mut self, node_id: Uuid) {
        assert!(
            self.nodes.iter().any(|node| node.id == node_id),
            "node must exist to be removed"
        );

        self.nodes.retain(|node| node.id != node_id);

        if self
            .selected_node_id
            .is_some_and(|selected| selected == node_id)
        {
            self.selected_node_id = None;
        }

        for node in &mut self.nodes {
            for input in &mut node.inputs {
                if let Some(connection) = &input.connection
                    && connection.node_id == node_id
                {
                    input.connection = None;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use graph::prelude::{TestFuncHooks, test_func_lib, test_graph as core_test_graph};

    #[test]
    fn graph_view_test() {
        let graph = build_test_view();
        assert!(graph.validate().is_ok());
    }

    #[test]
    fn graph_roundtrip() {
        assert_roundtrip(FileFormat::Json);
        assert_roundtrip(FileFormat::Yaml);
        assert_roundtrip(FileFormat::Lua);
    }

    fn build_test_view() -> GraphView {
        let graph = core_test_graph();
        let func_lib = test_func_lib(TestFuncHooks::default());
        GraphView::from_graph(&graph, &func_lib)
    }

    fn assert_roundtrip(format: FileFormat) {
        let graph = build_test_view();
        let serialized = graph.serialize(format);
        assert!(
            !serialized.trim().is_empty(),
            "serialized graph should not be empty"
        );
        let deserialized = GraphView::deserialize(format, &serialized)
            .expect("graph deserialization should succeed for test payload");
        assert!(deserialized.validate().is_ok());
        assert_eq!(
            graph.nodes.len(),
            deserialized.nodes.len(),
            "node counts should round-trip"
        );
        assert_eq!(
            graph.nodes[0].id, deserialized.nodes[0].id,
            "node ids should round-trip"
        );
        assert_eq!(graph.zoom, deserialized.zoom, "zoom should round-trip");
        assert_eq!(graph.pan, deserialized.pan, "pan should round-trip");
    }
}
