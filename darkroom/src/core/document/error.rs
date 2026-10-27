//! Structural validation errors for documents and their per-graph editor views.

use scenarium::{GraphValidationError, NodeId};

use crate::core::document::item_id::ItemId;
use crate::core::document::tab_ref::TabRef;
use crate::core::document::tie::TieId;
use crate::core::document::wire_key::WireKey;

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum GraphViewValidationError {
    #[error("graph viewport must have finite pan and positive finite zoom")]
    InvalidViewport,
    #[error("view item {node_id:?} position must be finite")]
    NonFinitePos { node_id: NodeId },
    #[error("view items must match the graph's nodes and the stubs")]
    NodeCount,
    #[error("graph view missing a position for node {node_id:?}")]
    MissingNode { node_id: NodeId },
    #[error("the route of {wire:?} names no wire the graph holds")]
    RouteToNoWire { wire: WireKey },
    #[error("the route of {wire:?}: {source}")]
    Route {
        wire: WireKey,
        #[source]
        source: RouteError,
    },
    #[error("the route of {wire:?} names {tie:?}, which is no tie")]
    RouteToNoTie { wire: WireKey, tie: TieId },
    #[error("tie {tie:?} holds no wire")]
    UnroutedTie { tie: TieId },
    #[error("tie {tie:?} position must be finite")]
    NonFiniteTiePos { tie: TieId },
    #[error("tie {tie:?} pinned angle must be in [0, 360)")]
    InvalidTieAngle { tie: TieId },
    #[error("selected item {item:?} is not in the view")]
    MissingSelectedItem { item: ItemId },
}

/// A rule a [`Route`](crate::core::document::route::Route) breaks.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum RouteError {
    #[error("a route names at least one tie")]
    Empty,
    #[error("a route names {tie:?} twice")]
    RepeatedTie { tie: TieId },
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
