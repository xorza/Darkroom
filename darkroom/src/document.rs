use crate::model::ViewGraph;

/// The thing being edited. Holds the `ViewGraph` (which itself owns the
/// core `Graph` + per-node view metadata). The `FuncLib` lives one level
/// up on `App` because it's runtime-owned (populated from builtins at
/// startup, shared across documents) — not document state.
#[derive(Debug, Default)]
pub struct Document {
    pub view_graph: ViewGraph,
}

impl Document {
    pub fn new(view_graph: ViewGraph) -> Self {
        Self { view_graph }
    }
}
