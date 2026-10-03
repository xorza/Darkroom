//! Structural validation errors for documents and their per-graph editor views.

use scenarium::{GraphValidationError, NodeId};

use crate::core::document::TabRef;

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
    #[error("open tab references a missing target {tab:?}")]
    MissingTab { tab: TabRef },
    /// The graph tab is the one that refuses to close, which is what keeps
    /// it reachable; a layout pinning another tab can lose it for good.
    #[error("the layout pins {tab:?} instead of the graph")]
    PinnedTab { tab: TabRef },
    /// The layout was saved by a dock with another seed, so none of its
    /// widget ids would be this editor's.
    #[error("the layout belongs to another dock")]
    ForeignDock,
}
