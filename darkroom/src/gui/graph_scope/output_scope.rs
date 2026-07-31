//! One output port of a node, with its type resolved through the graph.

use scenarium::{DataType, FuncOutput, OutputPort};

use crate::core::document::{PortKind, PortRef};
use crate::gui::graph_scope::node_scope::NodeScope;

/// One output port: what the func declares for it, and — for a wildcard —
/// the type the graph resolves it to.
#[derive(Clone, Copy, Debug)]
pub(crate) struct OutputScope<'a> {
    node: NodeScope<'a>,
    port_idx: usize,
    declared: &'a FuncOutput,
}

impl<'a> OutputScope<'a> {
    pub(super) fn new(node: NodeScope<'a>, port_idx: usize, declared: &'a FuncOutput) -> Self {
        Self {
            node,
            port_idx,
            declared,
        }
    }

    /// This port's address in the graph.
    pub(crate) fn port(self) -> OutputPort {
        OutputPort::new(self.node.id, self.port_idx)
    }

    /// This port's address in the canvas's glyph domains.
    pub(crate) fn port_ref(self) -> PortRef {
        PortRef {
            node_id: self.node.id,
            kind: PortKind::Output,
            port_idx: self.port_idx,
        }
    }

    pub(crate) fn name(self) -> &'a str {
        &self.declared.name
    }

    /// Port tooltip from the func's declaration; empty when it declares none.
    pub(crate) fn description(self) -> &'a str {
        self.declared.description.as_deref().unwrap_or_default()
    }

    /// The *resolved* type. A fixed output reports what it declares; a
    /// wildcard one (passthrough / reroute) reports the type followed through
    /// the wire it mirrors — `Any` until something is wired in.
    ///
    /// Resolved on read rather than from a table this scope carries: only a
    /// wildcard costs anything, the walk is its own chain rather than the
    /// graph, and a per-read answer cannot be a graph edit behind. Owned
    /// because a wildcard's answer is followed for, not stored anywhere to
    /// point at; [`DataType`] is a small enum, so the fixed case copies a
    /// discriminant.
    ///
    /// Re-validating downstream wires when an input changes is handled at
    /// edit time, not from here.
    pub(crate) fn ty(self) -> DataType {
        self.node
            .graph_scope
            .body()
            .output_type(self.node.graph_scope.library(), self.port())
    }
}
