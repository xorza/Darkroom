use scenarium::prelude::FuncLib;

use crate::model::ViewGraph;

/// The thing being edited. Bundles the `ViewGraph` (which itself
/// holds the core `Graph` + per-node view metadata) and the `FuncLib`
/// it resolves against. One owner so intents/scene/undo all read from
/// the same source without juggling parallel fields on `App`.
#[derive(Debug)]
pub struct Document {
    pub view_graph: ViewGraph,
    pub func_lib: FuncLib,
}

impl Document {
    pub fn new(view_graph: ViewGraph, func_lib: FuncLib) -> Self {
        Self {
            view_graph,
            func_lib,
        }
    }

    /// Shortcut into the core graph — equivalent to
    /// `self.view_graph.graph`, kept here so callers don't have to
    /// know the layering.
    #[allow(dead_code)]
    pub fn graph(&self) -> &scenarium::prelude::Graph {
        &self.view_graph.graph
    }
}
