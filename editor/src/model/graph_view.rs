use anyhow::{Result, anyhow, bail};
use common::{FileFormat, is_debug};
use graph::prelude::{Graph as CoreGraph, NodeId};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::ViewNode;

#[derive(Debug, Serialize, Deserialize)]
pub struct ViewGraph {
    pub graph: CoreGraph,
    pub view_nodes: Vec<ViewNode>,
    pub pan: egui::Vec2,
    pub zoom: f32,
    pub selected_node_id: Option<NodeId>,
}

impl Default for ViewGraph {
    fn default() -> Self {
        Self {
            graph: CoreGraph::default(),
            view_nodes: Vec::new(),
            pan: egui::Vec2::ZERO,
            zoom: 1.0,
            selected_node_id: None,
        }
    }
}

impl ViewGraph {
    pub fn from_graph(graph: &CoreGraph) -> Self {
        let mut nodes = Vec::with_capacity(graph.nodes.len());
        for (index, node) in graph.nodes.iter().enumerate() {
            let column = index % 3;
            let row = index / 3;
            let pos = egui::pos2(80.0 + 240.0 * column as f32, 120.0 + 180.0 * row as f32);

            nodes.push(ViewNode { id: node.id, pos });
        }

        let view_graph = Self {
            graph: graph.clone(),
            view_nodes: nodes,
            pan: egui::Vec2::ZERO,
            zoom: 1.0,
            selected_node_id: None,
        };
        view_graph
            .validate()
            .expect("graph view should be valid after conversion");
        view_graph
    }

    pub fn validate(&self) -> Result<()> {
        if !is_debug() {
            return Ok(());
        }

        self.graph.validate();

        if !self.zoom.is_finite() || self.zoom <= 0.0 {
            return Err(anyhow!("graph zoom must be finite and positive"));
        }
        if !self.pan.x.is_finite() || !self.pan.y.is_finite() {
            return Err(anyhow!("graph pan must be finite"));
        }

        let mut view_nodes = HashMap::new();
        for node in &self.view_nodes {
            if !node.pos.x.is_finite() || !node.pos.y.is_finite() {
                return Err(anyhow!("node position must be finite"));
            }
            let prior = view_nodes.insert(node.id, ());
            if prior.is_some() {
                return Err(anyhow!("duplicate node id detected"));
            }
        }

        if let Some(selected_node_id) = self.selected_node_id
            && !view_nodes.contains_key(&selected_node_id)
        {
            return Err(anyhow!("selected node id must exist in graph"));
        }

        let mut graph_nodes = HashMap::new();
        for node in self.graph.nodes.iter() {
            let prior = graph_nodes.insert(node.id, ());
            if prior.is_some() {
                return Err(anyhow!("duplicate node id detected in graph"));
            }
        }

        if view_nodes.len() != graph_nodes.len() {
            return Err(anyhow!("graph view node list must match graph nodes"));
        }
        for node_id in graph_nodes.keys() {
            if !view_nodes.contains_key(node_id) {
                return Err(anyhow!("graph view missing node position"));
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

        let view_graph = common::deserialize::<ViewGraph>(input, format)?;
        view_graph.validate()?;

        Ok(view_graph)
    }

    pub fn select_node(&mut self, node_id: NodeId) {
        assert!(
            self.view_nodes.iter().any(|node| node.id == node_id),
            "selected node view must exist in graph"
        );
        assert!(
            self.graph.by_id(&node_id).is_some(),
            "selected node must exist in graph data"
        );
        let node_index = self
            .view_nodes
            .iter()
            .position(|node| node.id == node_id)
            .expect("selected node view must exist in graph");
        if node_index + 1 != self.view_nodes.len() {
            let node = self.view_nodes.remove(node_index);
            self.view_nodes.push(node);
        }
        self.selected_node_id = Some(node_id);
    }

    pub fn remove_node(&mut self, node_id: NodeId) {
        assert!(
            self.view_nodes.iter().any(|node| node.id == node_id),
            "node must exist to be removed"
        );
        assert!(
            self.graph.by_id(&node_id).is_some(),
            "node must exist in graph data to be removed"
        );

        self.view_nodes.retain(|node| node.id != node_id);
        self.graph.remove_by_id(node_id);

        if self
            .selected_node_id
            .is_some_and(|selected| selected == node_id)
        {
            self.selected_node_id = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use graph::prelude::test_graph as core_test_graph;

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

    fn build_test_view() -> ViewGraph {
        let view_graph = core_test_graph();
        ViewGraph::from_graph(&view_graph)
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
        assert!(deserialized.validate().is_ok());
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
        assert_eq!(graph.zoom, deserialized.zoom, "zoom should round-trip");
        assert_eq!(graph.pan, deserialized.pan, "pan should round-trip");
    }
}
