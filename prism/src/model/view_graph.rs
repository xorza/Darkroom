use anyhow::{Result, bail};
use common::{SerdeFormat, is_debug, key_index_vec::KeyIndexVec};
use scenarium::prelude::{FuncLib, Graph as CoreGraph, NodeId};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::common::UiEquals;

use super::ViewNode;

// `pan`, `scale`, `selected_node_id` ride along in the serialized form.
// Open UX question: should they? A second viewer inherits the first's
// camera and selection on disk. Keeping as-is until "reopen exactly
// where I left off" vs "graph file is portable view-independent data"
// is decided. Cheap fix if we flip to the latter: add `#[serde(skip)]`.
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

    pub fn validate_with(&self, func_lib: &FuncLib) {
        if !is_debug() {
            return;
        }

        self.validate();

        self.graph.validate_with(func_lib);
    }

    pub fn serialize(&self, format: SerdeFormat) -> Vec<u8> {
        self.validate();
        common::serialize(self, format)
    }

    pub fn deserialize(format: SerdeFormat, input: &[u8]) -> Result<Self> {
        if input.is_empty() {
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
}

impl PartialEq for ViewGraph {
    fn eq(&self, other: &Self) -> bool {
        self.view_nodes == other.view_nodes
            && self.graph == other.graph
            && self.pan.ui_equals(other.pan)
            && self.scale.ui_equals(other.scale)
            && self.selected_node_id == other.selected_node_id
    }
}

impl Eq for ViewGraph {}

impl From<CoreGraph> for ViewGraph {
    fn from(graph: CoreGraph) -> Self {
        let mut view_nodes = KeyIndexVec::with_capacity(graph.nodes.len());
        for node in graph.nodes.iter() {
            let view_node = ViewNode::from(node);
            view_nodes.add(view_node);
        }

        Self {
            graph: graph.clone(),
            view_nodes,
            pan: egui::Vec2::ZERO,
            scale: 1.0,
            selected_node_id: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scenarium::prelude::test_graph as core_test_graph;

    #[test]
    fn graph_view_test() {
        let graph = build_test_view();
        graph.validate();
    }

    #[test]
    fn graph_roundtrip() {
        for format in SerdeFormat::all_formats_for_testing() {
            assert_roundtrip(format);
        }
    }

    fn build_test_view() -> ViewGraph {
        core_test_graph().into()
    }

    fn assert_roundtrip(format: SerdeFormat) {
        let graph = build_test_view();
        let serialized = graph.serialize(format);
        assert!(
            !serialized.is_empty(),
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
