//! Structural validation for documents and their per-graph editor views.

use scenarium::{Graph as CoreGraph, GraphValidationError, NodeId};

use crate::core::document::dock::DockValidationError;
use crate::core::document::{Document, GraphView, TabRef, tab_alive};

#[derive(Debug, thiserror::Error)]
pub(crate) enum GraphViewValidationError {
    #[error("graph viewport must have finite pan and positive finite zoom")]
    InvalidViewport,
    #[error("view item {item:?} position must be finite")]
    NonFinitePosition { item: NodeId },
    #[error("view node items must match graph nodes")]
    NodeCount,
    #[error("graph view missing a position for node {node_id:?}")]
    MissingNode { node_id: NodeId },
    #[error("selected item {item:?} has no view item")]
    MissingSelectedItem { item: NodeId },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum DocumentValidationError {
    #[error(transparent)]
    Graph(#[from] GraphValidationError),
    #[error("main view: {source}")]
    MainView {
        #[source]
        source: GraphViewValidationError,
    },
    #[error(transparent)]
    Dock(#[from] DockValidationError),
    #[error("open tab references a missing target {tab:?}")]
    MissingTab { tab: TabRef },
}

impl GraphView {
    fn validate(&self, graph: &CoreGraph) -> Result<(), GraphViewValidationError> {
        if !self.viewport.is_valid() {
            return Err(GraphViewValidationError::InvalidViewport);
        }

        // IndexMap guarantees unique keys, so counts plus reverse membership
        // prove the graph and view contain exactly the same node and pin sets.
        let mut node_items = 0usize;
        for (key, position) in &self.item_placements {
            if !position.is_finite() {
                return Err(GraphViewValidationError::NonFinitePosition { item: *key });
            }
            node_items += 1;
        }
        if node_items != graph.len() {
            return Err(GraphViewValidationError::NodeCount);
        }
        for node in graph.iter() {
            if !self.item_placements.contains_key(&node.id) {
                return Err(GraphViewValidationError::MissingNode { node_id: node.id });
            }
        }
        for key in &self.selected {
            if !self.item_placements.contains_key(key) {
                return Err(GraphViewValidationError::MissingSelectedItem { item: *key });
            }
        }
        Ok(())
    }
}

impl Document {
    /// Full structural validation for untrusted documents.
    pub(crate) fn validate(&self) -> Result<(), DocumentValidationError> {
        self.graph.validate()?;
        self.main_view
            .validate(&self.graph)
            .map_err(|source| DocumentValidationError::MainView { source })?;

        self.layout.validate()?;
        for tab in self.layout.all_tabs() {
            if !tab_alive(&self.graph, tab) {
                return Err(DocumentValidationError::MissingTab { tab });
            }
        }
        Ok(())
    }
}
