//! [`Binding`]: what a consumer input port is wired to.

use ::serde::{Deserialize, Serialize};

use crate::data::const_value::ConstValue;
use crate::graph::identity::{InputPort, NodeId, OutputPort};

/// What a consumer input port is wired to; an absent port in `Graph.bindings` is unbound.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Binding {
    Const(ConstValue),
    Wire(OutputPort),
}

impl Binding {
    /// A data binding wired to producer `node_id`'s output port `port_idx`.
    pub const fn wire(node_id: NodeId, port_idx: usize) -> Self {
        Self::Wire(OutputPort::new(node_id, port_idx))
    }

    /// Whether this binding, on `port`, touches `node_id` from either end: it lands on
    /// that node, or it reads from it.
    pub(crate) fn touches(&self, port: InputPort, node_id: NodeId) -> bool {
        port.node_id == node_id || matches!(self, Self::Wire(src) if src.node_id == node_id)
    }
}

impl From<OutputPort> for Binding {
    fn from(value: OutputPort) -> Self {
        Self::Wire(value)
    }
}

impl From<ConstValue> for Binding {
    fn from(value: ConstValue) -> Self {
        Self::Const(value)
    }
}
