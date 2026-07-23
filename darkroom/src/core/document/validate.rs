//! Structural validation for documents and their per-graph editor views.

use common::is_debug;
use scenarium::{Graph as CoreGraph, GraphId, GraphValidationError, NodeId, OutputPort};

use crate::core::document::dock::DockValidationError;
use crate::core::document::{Document, GraphView, ItemRef, TabRef, tab_alive};

#[derive(Debug, thiserror::Error)]
pub(crate) enum GraphViewValidationError {
    #[error("graph viewport must have finite pan and positive finite zoom")]
    InvalidViewport,
    #[error("view item {item:?} position must be finite")]
    NonFinitePosition { item: ItemRef },
    #[error("view item references output {port:?}, which isn't pinned")]
    UnpinnedOutput { port: OutputPort },
    #[error("view node items must match graph nodes")]
    NodeCount,
    #[error("graph view missing a position for node {node_id:?}")]
    MissingNode { node_id: NodeId },
    #[error("pinned output {port:?} must have a view item")]
    MissingPinnedOutput { port: OutputPort },
    #[error("selected item {item:?} has no view item")]
    MissingSelectedItem { item: ItemRef },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum DocumentValidationError {
    #[error(transparent)]
    Graph(#[from] GraphValidationError),
    #[error("entry graph cannot expose an interface")]
    EntryInterface,
    #[error("entry graph cannot contain interface boundary nodes")]
    EntryBoundaryNodes,
    #[error("main view: {source}")]
    MainView {
        #[source]
        source: GraphViewValidationError,
    },
    #[error("local_views entry references missing local graph {graph_id:?}")]
    MissingLocalGraph { graph_id: GraphId },
    #[error("graph {graph_id:?} view: {source}")]
    LocalView {
        graph_id: GraphId,
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
            match key {
                ItemRef::Node(_) => node_items += 1,
                ItemRef::Pin(port) if !graph.is_output_pinned(*port) => {
                    return Err(GraphViewValidationError::UnpinnedOutput { port: *port });
                }
                ItemRef::Pin(_) => {}
            }
        }
        if node_items != graph.len() {
            return Err(GraphViewValidationError::NodeCount);
        }
        for node in graph.iter() {
            if !self.item_placements.contains_key(&ItemRef::Node(node.id)) {
                return Err(GraphViewValidationError::MissingNode { node_id: node.id });
            }
        }
        for port in graph.pinned_outputs() {
            if !self.item_placements.contains_key(&ItemRef::Pin(port)) {
                return Err(GraphViewValidationError::MissingPinnedOutput { port });
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
        if !self.graph.inputs.is_empty()
            || !self.graph.outputs.is_empty()
            || !self.graph.events.is_empty()
        {
            return Err(DocumentValidationError::EntryInterface);
        }
        if self.graph.iter().any(|node| node.kind.is_boundary()) {
            return Err(DocumentValidationError::EntryBoundaryNodes);
        }

        self.main_view
            .validate(&self.graph)
            .map_err(|source| DocumentValidationError::MainView { source })?;
        for (id, view) in &self.local_views {
            let graph = self
                .graph
                .graphs
                .get(id)
                .ok_or(DocumentValidationError::MissingLocalGraph { graph_id: *id })?;
            view.validate(graph)
                .map_err(|source| DocumentValidationError::LocalView {
                    graph_id: *id,
                    source,
                })?;
        }

        self.layout.validate()?;
        for tab in self.layout.all_tabs() {
            if !tab_alive(&self.graph, tab) {
                return Err(DocumentValidationError::MissingTab { tab });
            }
        }
        Ok(())
    }

    /// Debug-only assert form of [`Self::validate`].
    pub(crate) fn validate_debug(&self) {
        if !is_debug() {
            return;
        }
        self.validate()
            .expect("document structural invariant violated");
    }
}
