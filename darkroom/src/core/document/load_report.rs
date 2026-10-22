//! [`LoadReport`]: what a load changed to make a decoded document consistent
//! with itself and the library.

use std::fmt;
use std::fmt::{Display, Formatter};

use scenarium::{BindingEntry, NodeId, Repair, Subscription, Unresolved};

use crate::core::status_log::{StatusFamily, StatusLog};

/// Every change a load made, in the order it made them. Empty for a document
/// that was already consistent.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct LoadReport {
    pub(crate) repairs: Vec<DocumentRepair>,
}

/// One change a load made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DocumentRepair {
    /// A stub saved under the id of a graph node, which the graph keeps.
    DuplicateStub {
        node_id: NodeId,
    },
    /// A stub whose func resolves again, attached to the graph.
    Reattached {
        node_id: NodeId,
    },
    /// A reattached stub's binding whose other end is gone, or whose input
    /// the graph has bound since.
    StubBindingDropped {
        node_id: NodeId,
        entry: BindingEntry,
    },
    /// A reattached stub's subscription whose other end is gone.
    StubSubscriptionDropped {
        node_id: NodeId,
        subscription: Subscription,
    },
    /// A graph repair: wiring or a node the library cannot hold.
    Graph(Repair),
    /// A node the library cannot resolve, kept as a stub.
    Stubbed {
        node_id: NodeId,
        reason: Unresolved,
    },
    ViewportReset,
    /// A placement for no node or stub.
    PlacementDropped {
        node_id: NodeId,
    },
    /// A non-finite position, moved to the origin.
    PositionReset {
        node_id: NodeId,
    },
    /// A node or stub with no placement, placed at the origin.
    Placed {
        node_id: NodeId,
    },
    /// A selection entry with no placement.
    SelectionDropped {
        node_id: NodeId,
    },
    /// A layout that pins another tab, or belongs to another dock.
    LayoutReset,
    /// A viewer tab on a node that is not a preview node in the graph.
    ViewerClosed {
        node_id: NodeId,
    },
}

impl LoadReport {
    pub(crate) const fn is_empty(&self) -> bool {
        self.repairs.is_empty()
    }

    /// Log each change, and put one line on the status bar that says how many
    /// there were. The next save clears the line: a save accepts the changes.
    pub(crate) fn report(&self, status: &mut StatusLog) {
        if self.is_empty() {
            return;
        }
        for repair in &self.repairs {
            tracing::warn!(target: "darkroom::load", "{repair}");
        }
        status.warn(
            StatusFamily::Document,
            format!(
                "the load changed {} item(s) to fit the library; save to keep the changes",
                self.repairs.len()
            ),
        );
    }
}

impl Display for DocumentRepair {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateStub { node_id } => {
                write!(f, "removed a stub of {node_id:?}, which the graph holds")
            }
            Self::Reattached { node_id } => {
                write!(f, "attached the stub {node_id:?} again: its func resolves")
            }
            Self::StubBindingDropped { node_id, entry } => write!(
                f,
                "removed {:?} on {:?} from the stub {node_id:?}: its other end is gone, or the \
                 input is bound",
                entry.binding, entry.port
            ),
            Self::StubSubscriptionDropped {
                node_id,
                subscription,
            } => write!(
                f,
                "removed {subscription:?} from the stub {node_id:?}: its other end is gone"
            ),
            Self::Graph(repair) => repair.fmt(f),
            Self::Stubbed { node_id, reason } => {
                write!(f, "kept {node_id:?} as a stub: {reason}")
            }
            Self::ViewportReset => f.write_str("reset the viewport"),
            Self::PlacementDropped { node_id } => {
                write!(
                    f,
                    "removed the placement of {node_id:?}, which names no node"
                )
            }
            Self::PositionReset { node_id } => {
                write!(
                    f,
                    "moved {node_id:?} to the origin: its position is not finite"
                )
            }
            Self::Placed { node_id } => {
                write!(f, "placed {node_id:?} at the origin: it had no placement")
            }
            Self::SelectionDropped { node_id } => {
                write!(
                    f,
                    "removed {node_id:?} from the selection: it has no placement"
                )
            }
            Self::LayoutReset => f.write_str("reset the pane layout"),
            Self::ViewerClosed { node_id } => {
                write!(
                    f,
                    "closed the viewer of {node_id:?}, which is no preview node"
                )
            }
        }
    }
}
