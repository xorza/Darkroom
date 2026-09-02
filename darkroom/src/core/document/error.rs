//! Structural validation errors for documents and their per-graph editor views.

use scenarium::{GraphValidationError, NodeId};

use palantir::DockError;

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
    #[error("dock: {0}")]
    Dock(#[from] DockError<TabRef>),
    #[error("open tab references a missing target {tab:?}")]
    MissingTab { tab: TabRef },
}
