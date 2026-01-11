use anyhow::{Result, bail};
use common::{FileFormat, is_debug, key_index_vec::KeyIndexVec};
use graph::graph::{Binding, Node};
use graph::prelude::{Graph as CoreGraph, NodeId};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::common::UiEquals;

use super::ViewNode;
use crate::gui::graph_ui_interaction::GraphUiAction;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IncomingConnection {
    pub node_id: NodeId,
    pub input_idx: usize,
    pub binding: Binding,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IncomingEvent {
    pub node_id: NodeId,
    pub event_idx: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ViewGraph {
    pub graph: CoreGraph,
    pub view_nodes: KeyIndexVec<NodeId, ViewNode>,
    pub pan: egui::Vec2,
    pub scale: f32,
    pub selected_node_id: Option<NodeId>,
}

impl Default for ViewGraph {
    fn default() -> Self {
        Self {
            graph: CoreGraph::default(),
            view_nodes: KeyIndexVec::default(),
            pan: egui::Vec2::ZERO,
            scale: 1.0,
            selected_node_id: None,
        }
    }
}

impl ViewGraph {
    pub fn validate(&self) {
        if !is_debug() {
            return;
        }

        self.graph.validate();

        assert!(
            self.scale.is_finite() && self.scale > 0.0,
            "graph zoom must be finite and positive"
        );
        assert!(
            self.pan.x.is_finite() && self.pan.y.is_finite(),
            "graph pan must be finite"
        );

        let mut view_nodes = HashMap::new();
        for node in self.view_nodes.iter() {
            assert!(
                node.pos.x.is_finite() && node.pos.y.is_finite(),
                "node position must be finite"
            );
            let prior = view_nodes.insert(node.id, ());
            assert!(prior.is_none(), "duplicate node id detected");
        }

        if let Some(selected_node_id) = self.selected_node_id
            && !view_nodes.contains_key(&selected_node_id)
        {
            panic!("selected node id must exist in graph");
        }

        let mut graph_nodes = HashMap::new();
        for node in self.graph.nodes.iter() {
            let prior = graph_nodes.insert(node.id, ());
            assert!(prior.is_none(), "duplicate node id detected in graph");
        }

        assert_eq!(
            view_nodes.len(),
            graph_nodes.len(),
            "graph view node list must match graph nodes"
        );
        for node_id in graph_nodes.keys() {
            if !view_nodes.contains_key(node_id) {
                panic!("graph view missing node position");
            }
        }
    }

    pub fn serialize(&self, format: FileFormat) -> String {
        self.validate();
        common::serialize(self, format)
    }

    pub fn deserialize(format: FileFormat, input: &str) -> Result<Self> {
        if input.trim().is_empty() {
            bail!("graph input is empty");
        }

        let view_graph = common::deserialize::<ViewGraph>(input, format)?;
        view_graph.validate();

        Ok(view_graph)
    }

    pub fn remove_node(&mut self, node_id: &NodeId) {
        self.view_nodes.retain(|node| node.id != *node_id);
        self.graph.remove_by_id(*node_id);

        if self
            .selected_node_id
            .is_some_and(|selected| selected == *node_id)
        {
            self.selected_node_id = None;
        }
    }

    pub fn removal_action(&self, node_id: &NodeId) -> GraphUiAction {
        let view_node = self
            .view_nodes
            .by_key(node_id)
            .expect("remove node expects a view node")
            .clone();
        let node = self
            .graph
            .by_id(node_id)
            .expect("remove node expects a graph node")
            .clone();
        let mut incoming_connections = Vec::new();
        let mut incoming_events = Vec::new();
        for node in self.graph.nodes.iter() {
            for (input_idx, input) in node.inputs.iter().enumerate() {
                let Binding::Bind(binding) = &input.binding else {
                    continue;
                };
                if binding.target_id == *node_id {
                    incoming_connections.push(IncomingConnection {
                        node_id: node.id,
                        input_idx,
                        binding: input.binding.clone(),
                    });
                }
            }
            for (event_idx, event) in node.events.iter().enumerate() {
                if event.subscribers.contains(node_id) {
                    incoming_events.push(IncomingEvent {
                        node_id: node.id,
                        event_idx,
                    });
                }
            }
        }
        GraphUiAction::NodeRemoved {
            view_node,
            node,
            incoming_connections,
            incoming_events,
        }
    }
}

impl PartialEq for ViewGraph {
    fn eq(&self, other: &Self) -> bool {
        self.view_nodes == other.view_nodes
            && self.graph == other.graph
            && self.pan.ui_equals(&other.pan)
            && self.scale.ui_equals(&other.scale)
            && self.selected_node_id == other.selected_node_id
    }
}

impl Eq for ViewGraph {}

impl From<CoreGraph> for ViewGraph {
    fn from(graph: CoreGraph) -> Self {
        let mut view_nodes = KeyIndexVec::with_capacity(graph.nodes.len());
        for (idx, node) in graph.nodes.iter().enumerate() {
            let column = idx % 3;
            let row = idx / 3;
            let pos = egui::pos2(80.0 + 240.0 * column as f32, 120.0 + 180.0 * row as f32);

            let mut view_node = ViewNode::from(node);
            view_node.pos = pos;
            view_nodes.add(view_node);
        }

        let view_graph = Self {
            graph: graph.clone(),
            view_nodes,
            pan: egui::Vec2::ZERO,
            scale: 1.0,
            selected_node_id: None,
        };
        view_graph.validate();
        view_graph
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use graph::prelude::test_graph as core_test_graph;

    #[test]
    fn graph_view_test() {
        let graph = build_test_view();
        graph.validate();
    }

    #[test]
    fn graph_roundtrip() {
        assert_roundtrip(FileFormat::Json);
        assert_roundtrip(FileFormat::Yaml);
        assert_roundtrip(FileFormat::Lua);
    }

    fn build_test_view() -> ViewGraph {
        core_test_graph().into()
    }

    fn assert_roundtrip(format: FileFormat) {
        let graph = build_test_view();
        let serialized = graph.serialize(format);
        assert!(
            !serialized.trim().is_empty(),
            "serialized graph should not be empty"
        );
        let deserialized = ViewGraph::deserialize(format, &serialized)
            .expect("graph deserialization should succeed for test payload");
        deserialized.validate();
        assert_eq!(
            graph.view_nodes.len(),
            deserialized.view_nodes.len(),
            "node view counts should round-trip"
        );
        assert_eq!(
            graph.view_nodes[0].id, deserialized.view_nodes[0].id,
            "node view ids should round-trip"
        );
        assert_eq!(
            graph.graph.nodes.len(),
            deserialized.graph.nodes.len(),
            "graph nodes should round-trip"
        );
        assert_eq!(graph.scale, deserialized.scale, "zoom should round-trip");
        assert_eq!(graph.pan, deserialized.pan, "pan should round-trip");
    }
}
