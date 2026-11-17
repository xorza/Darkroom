//! Structural validation errors for documents and their per-graph editor views.

use scenarium::{CompoundId, GraphId, GraphValidationError, InstancePath, NodeAddr, NodeId};

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
    #[error("view items must match the graph's nodes and the ghost nodes")]
    NodeCount,
    #[error("graph view missing a position for node {node_id:?}")]
    MissingNode { node_id: NodeId },
    #[error("the route of {wire:?} names no wire the graph holds")]
    RouteToNoWire { wire: WireKey },
    #[error("the route of {wire:?}")]
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
    #[error("graph {graph:?}")]
    Graph {
        graph: GraphId,
        #[source]
        source: GraphValidationError,
    },
    #[error("the view of {graph:?}")]
    View {
        graph: GraphId,
        #[source]
        source: GraphViewValidationError,
    },
    #[error("compound {compound:?} has no side, or a side names no compound")]
    SideMismatch { compound: CompoundId },
    #[error("compound {compound:?} is shown through {path:?}, which names no instance of it")]
    StaleThrough {
        compound: CompoundId,
        path: InstancePath,
    },
    #[error("open tab {tab:?} shows nothing the document holds")]
    MissingTab { tab: TabRef },
    #[error("the active graph {graph:?} is no graph of the set")]
    ActiveMissing { graph: GraphId },
    #[error("node {node:?} holds the relative path {path:?}, which memory holds resolved")]
    RelativePath { node: NodeAddr, path: String },
}
