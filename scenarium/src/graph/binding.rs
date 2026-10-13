//! [`Binding`]: what a consumer input port is wired to.

use ::serde::{Deserialize, Serialize};

use crate::ConstValue;
use crate::graph::identity::{InputPort, NodeId, OutputPort};

/// What a consumer input port is wired to. Stored sparsely: `Graph.bindings`
/// only holds these values, and an absent port is unbound.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Binding {
    Const(ConstValue),
    Bind(OutputPort),
}

impl Binding {
    /// A data binding wired to producer `node_id`'s output port `port_idx`.
    pub const fn bind(node_id: NodeId, port_idx: usize) -> Self {
        Self::Bind(OutputPort::new(node_id, port_idx))
    }

    /// Whether this edge — the binding on `port` — touches `node_id` from
    /// either end: it lands on that node, or it reads from it. How a detach
    /// finds the wiring it has to take with it, and how the record it
    /// produced proves it took only that.
    pub(crate) fn touches(&self, port: InputPort, node_id: NodeId) -> bool {
        port.node_id == node_id || matches!(self, Self::Bind(src) if src.node_id == node_id)
    }
}

impl From<OutputPort> for Binding {
    fn from(value: OutputPort) -> Self {
        Self::Bind(value)
    }
}

impl From<ConstValue> for Binding {
    fn from(value: ConstValue) -> Self {
        Self::Const(value)
    }
}
