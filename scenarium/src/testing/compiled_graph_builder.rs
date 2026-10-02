//! [`CompiledGraphBuilder`]: a program of bare nodes for a host's tests.

use std::sync::Arc;

use crate::execution::compile::compiled_graph::{CompiledGraph, ExecutionNode};
use crate::graph::identity::NodeId;

/// A [`CompiledGraph`] of bare nodes, for a host test that only has to
/// resolve authored ids against a program.
#[derive(Debug, Default)]
pub struct CompiledGraphBuilder {
    node_ids: Vec<NodeId>,
}

impl CompiledGraphBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add the execution node an authored node became. One node per authored
    /// id now that nothing dissolves, so the two are the same identity.
    pub fn insert_node(&mut self, node_id: NodeId) {
        self.node_ids.push(node_id);
    }

    /// Sorted on the way in, like the real walk: a fixture that placed its
    /// nodes in insertion order would let a test pass against an index
    /// layout no compile produces.
    pub fn build(mut self) -> Arc<CompiledGraph> {
        self.node_ids.sort();
        let mut compiled = CompiledGraph::default();
        for node_id in self.node_ids {
            // These nodes declare no ports, so the default is exactly the
            // bare node a fixture wants.
            compiled.push(node_id, ExecutionNode::default());
        }
        Arc::new(compiled)
    }
}
