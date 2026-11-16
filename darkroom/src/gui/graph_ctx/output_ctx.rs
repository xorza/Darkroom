//! One output port of a node.

use scenarium::{DataType, OutputDecl, OutputPort};

use crate::gui::graph_ctx::node_ctx::NodeCtx;
use crate::gui::port_ref::PortRef;

/// One output port: what the func declares for it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct OutputCtx<'a> {
    node: NodeCtx<'a>,
    port_idx: usize,
    declared: &'a OutputDecl,
}

impl<'a> OutputCtx<'a> {
    pub(super) const fn new(node: NodeCtx<'a>, port_idx: usize, declared: &'a OutputDecl) -> Self {
        Self {
            node,
            port_idx,
            declared,
        }
    }

    /// This port's address in the graph.
    pub(crate) const fn port(self) -> OutputPort {
        OutputPort::new(self.node.node_id, self.port_idx)
    }

    /// This port's address in the canvas's glyph domains.
    pub(crate) const fn port_ref(self) -> PortRef {
        PortRef::output(self.node.node_id, self.port_idx)
    }

    pub(crate) fn name(self) -> &'a str {
        &self.declared.name
    }

    /// Port tooltip from the func's declaration; empty when it declares none.
    pub(crate) fn description(self) -> &'a str {
        self.declared.description.as_deref().unwrap_or_default()
    }

    pub(crate) const fn data_type(self) -> DataType {
        self.declared.data_type
    }
}
