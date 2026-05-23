use anyhow::{Result, bail};
use common::{SerdeFormat, is_debug, key_index_vec::KeyIndexVec};
use glam::Vec2;
use scenarium::prelude::{Binding, FuncLib, Graph as CoreGraph, NodeId};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::model::ViewNode;

/// The thing being edited: the core `Graph`, per-node view metadata,
/// and viewport state (all persisted). The `FuncLib` it resolves
/// against lives one level up on `App` because it's runtime-owned
/// (populated from builtins at startup, shared across documents).
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub graph: CoreGraph,
    pub view_nodes: KeyIndexVec<NodeId, ViewNode>,
    pub pan: Vec2,
    pub scale: f32,
    pub selected_node_id: Option<NodeId>,
}

impl Default for Document {
    fn default() -> Self {
        Self {
            graph: CoreGraph::default(),
            view_nodes: KeyIndexVec::default(),
            pan: Vec2::ZERO,
            scale: 1.0,
            selected_node_id: None,
        }
    }
}

#[allow(dead_code)]
impl Document {
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
        for node in self.graph.iter() {
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
            bail!("document input is empty");
        }

        let doc = common::deserialize::<Document>(input, format)?;
        doc.validate();

        Ok(doc)
    }

    /// Assign positions to `view_nodes` using topological-depth columns:
    /// nodes with no bound inputs go in column 0, downstream nodes shift
    /// right by one column per max-upstream-depth. Within a column, stack
    /// vertically in graph insertion order.
    pub fn auto_layout(&mut self, col_spacing: f32, row_spacing: f32, origin: Vec2) {
        let mut depth: HashMap<NodeId, u32> = HashMap::new();
        for node in self.graph.iter() {
            let d = node
                .inputs
                .iter()
                .filter_map(|inp| match &inp.binding {
                    Binding::Bind(addr) => depth.get(&addr.target_id).copied().map(|d| d + 1),
                    _ => None,
                })
                .max()
                .unwrap_or(0);
            depth.insert(node.id, d);
        }

        let mut row_in_col: HashMap<u32, u32> = HashMap::new();
        for view_node in self.view_nodes.iter_mut() {
            let d = depth.get(&view_node.id).copied().unwrap_or(0);
            let row = row_in_col.entry(d).or_insert(0);
            view_node.pos = origin + Vec2::new(d as f32 * col_spacing, *row as f32 * row_spacing);
            *row += 1;
        }
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

impl Eq for Document {}

impl From<CoreGraph> for Document {
    fn from(graph: CoreGraph) -> Self {
        let mut view_nodes = KeyIndexVec::with_capacity(graph.len());
        for node in graph.iter() {
            let view_node = ViewNode::from(node);
            view_nodes.add(view_node);
        }

        Self {
            graph: graph.clone(),
            view_nodes,
            pan: Vec2::ZERO,
            scale: 1.0,
            selected_node_id: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scenarium::testing::test_graph as core_test_graph;

    #[test]
    fn document_validates() {
        let doc = build_test_doc();
        doc.validate();
    }

    #[test]
    fn document_roundtrip() {
        for format in SerdeFormat::all_formats_for_testing() {
            assert_roundtrip(format);
        }
    }

    fn build_test_doc() -> Document {
        core_test_graph().into()
    }

    fn assert_roundtrip(format: SerdeFormat) {
        let doc = build_test_doc();
        let serialized = doc.serialize(format);
        assert!(
            !serialized.is_empty(),
            "serialized document should not be empty"
        );
        let deserialized = Document::deserialize(format, &serialized)
            .expect("document deserialization should succeed for test payload");
        deserialized.validate();
        assert_eq!(
            doc.view_nodes.len(),
            deserialized.view_nodes.len(),
            "node view counts should round-trip"
        );
        assert_eq!(
            doc.view_nodes[0].id, deserialized.view_nodes[0].id,
            "node view ids should round-trip"
        );
        assert_eq!(
            doc.graph.len(),
            deserialized.graph.len(),
            "graph nodes should round-trip"
        );
        assert_eq!(doc.scale, deserialized.scale, "zoom should round-trip");
        assert_eq!(doc.pan, deserialized.pan, "pan should round-trip");
    }
}
