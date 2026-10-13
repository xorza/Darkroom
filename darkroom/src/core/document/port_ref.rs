//! [`PortRef`]: one data port's identity in the graph.

use scenarium::{InputPort, NodeId, OutputPort};
use serde::{Deserialize, Serialize};

use crate::core::document::node_key::NodeKey;

/// Whether a port consumes a binding (`Input`) or produces a value
/// (`Output`). Scoped to the data-port subset until Trigger/Event are
/// reintroduced. `Input` ports live in the left column, `Output` in
/// the right; `opposite` flips between them for snap-target tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum PortKind {
    Input,
    Output,
}

impl PortKind {
    pub(crate) const fn opposite(self) -> Self {
        match self {
            Self::Input => Self::Output,
            Self::Output => Self::Input,
        }
    }
}

/// One port's identity in the graph. Domain-keyed so UI passes can derive
/// its `WidgetId` (see `crate::gui::pane::graph::node::port_row::port_circle_wid`)
/// without threading a cache, and serializable so a persisted tab
/// ([`TabRef::ImageViewer`](crate::core::document::tab_ref::TabRef::ImageViewer))
/// can bind to it. Node ids are unique across the whole document, so no graph
/// ref is needed alongside — enforced by
/// [`Document::validate`](crate::core::document::Document::validate).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct PortRef {
    pub(crate) node_id: NodeId,
    pub(crate) kind: PortKind,
    pub(crate) port_idx: usize,
}

impl NodeKey for PortRef {
    fn node(self) -> NodeId {
        self.node_id
    }
}

impl PortRef {
    /// `node_id`'s `port_idx`th input — the left-column counterpart of the
    /// graph's [`InputPort`].
    pub(crate) const fn input(node_id: NodeId, port_idx: usize) -> Self {
        Self {
            node_id,
            kind: PortKind::Input,
            port_idx,
        }
    }

    /// `node_id`'s `port_idx`th output — the right-column counterpart of the
    /// graph's [`OutputPort`].
    pub(crate) const fn output(node_id: NodeId, port_idx: usize) -> Self {
        Self {
            node_id,
            kind: PortKind::Output,
            port_idx,
        }
    }
}

impl From<InputPort> for PortRef {
    fn from(port: InputPort) -> Self {
        Self::input(port.node_id, port.port_idx)
    }
}

impl From<OutputPort> for PortRef {
    fn from(port: OutputPort) -> Self {
        Self::output(port.node_id, port.port_idx)
    }
}
