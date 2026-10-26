//! Structural validation errors for documents and their per-graph editor views.

use scenarium::{GraphValidationError, NodeId};

use crate::core::document::tab_ref::TabRef;

#[derive(Debug, thiserror::Error)]
pub(crate) enum GraphViewValidationError {
    #[error("graph viewport must have finite pan and positive finite zoom")]
    InvalidViewport,
    #[error("view item {node_id:?} position must be finite")]
    NonFinitePos { node_id: NodeId },
    #[error("view items must match the graph's nodes and the stubs")]
    NodeCount,
    #[error("graph view missing a position for node {node_id:?}")]
    MissingNode { node_id: NodeId },
    #[error("selected item {node_id:?} has no view item")]
    MissingSelectedItem { node_id: NodeId },
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
    #[error("open tab {tab:?} names no preview node in the graph")]
    MissingTab { tab: TabRef },
    /// The graph tab is the one that refuses to close, which is what keeps
    /// it reachable; a layout pinning another tab can lose it for good.
    #[error("the layout pins {tab:?} instead of the graph")]
    PinnedTab { tab: TabRef },
    /// The layout was saved by a dock with another seed, so none of its
    /// widget ids would be this editor's.
    #[error("the layout belongs to another dock")]
    ForeignDock,
    #[error("node {node_id:?} is both a graph node and a stub")]
    StubInGraph { node_id: NodeId },
    #[error("node {node_id:?} holds the relative path {path:?}, which memory holds absolute")]
    RelativePath { node_id: NodeId, path: String },
}
