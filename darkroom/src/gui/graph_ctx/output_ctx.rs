//! One output port of a node, with its type resolved through the graph.

use scenarium::{DataType, FuncOutput, OutputPort};

use crate::core::document::PortRef;
use crate::gui::graph_ctx::node_scope::NodeScope;

/// One output port: what the func declares for it, and — for a wildcard —
/// the type the graph resolves it to.
#[derive(Clone, Copy, Debug)]
pub(crate) struct OutputCtx<'a> {
    node: NodeScope<'a>,
    port_idx: usize,
    declared: &'a FuncOutput,
}

impl<'a> OutputCtx<'a> {
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
        PortRef::output(self.node.id, self.port_idx)
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
    /// A lookup, not a walk: the scope carries the whole graph's resolved
    /// types, refreshed wherever it is composed, so a chain several ports read
    /// is followed once per frame rather than once per read — and reading one
    /// obeys the scope's rule that no accessor traverses the graph.
    ///
    /// Owned because a wildcard's answer is followed for, not stored anywhere
    /// the caller may keep; [`DataType`] is a small enum, so the fixed case
    /// copies a discriminant.
    ///
    /// Re-validating downstream wires when an input changes is handled at
    /// edit time, not from here.
    ///
    /// # Panics
    ///
    /// If the table does not cover this port. That is not drift and not an
    /// unresolvable chain — both of those are *present* and `Any`. It means
    /// the table was resolved against a different graph or library than the
    /// scope carries, since a port only reaches here off a func the same
    /// `node_func` lookup resolved, at an index that func declares. Degrading
    /// to `Any` would paint a stale port a plausible colour and let a scope
    /// composed without a refresh go unnoticed.
    pub(crate) fn ty(self) -> DataType {
        self.node
            .graph_ctx
            .output_types()
            .get(self.port())
            .expect("the scope's table is resolved against the graph it carries")
            .clone()
    }
}
