//! One input port of a node, with its declaration and current binding.

use scenarium::{
    Binding, ConstValue, DataType, GhostBinding, GraphId, InputDecl, InputPort, Library, Numbered,
    ValueDomain,
};

use crate::gui::graph_ctx::node_ctx::NodeCtx;
use crate::gui::port_ref::PortRef;

/// One input port: what the func declares for it, and what the graph has
/// bound to it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct InputCtx<'a> {
    node: NodeCtx<'a>,
    /// Where the port stands among the node's inputs, the row it draws on.
    idx: usize,
    declared: &'a Numbered<InputDecl>,
}

impl<'a> InputCtx<'a> {
    pub(super) const fn new(
        node: NodeCtx<'a>,
        idx: usize,
        declared: &'a Numbered<InputDecl>,
    ) -> Self {
        Self {
            node,
            idx,
            declared,
        }
    }

    /// This port's address in the graph.
    pub(crate) const fn port(self) -> InputPort {
        InputPort::new(self.node.node_id, self.declared.id)
    }

    /// The node the port is on.
    pub(crate) const fn node(self) -> NodeCtx<'a> {
        self.node
    }

    /// The graph the port's node is in.
    pub(crate) const fn graph(self) -> GraphId {
        self.node.graph_ctx.graph()
    }

    /// The library the port's declaration resolves against.
    pub(crate) const fn library(self) -> &'a Library {
        self.node.graph_ctx.library()
    }

    /// This port's address in the canvas's glyph domains.
    pub(crate) const fn port_ref(self) -> PortRef {
        PortRef::input(self.node.node_id, self.declared.id)
    }

    /// Whether the input takes no edit: its graph is a compound that is locked or follows the
    /// library, whose every edit the gate refuses.
    pub(crate) fn read_only(self) -> bool {
        let graph_ctx = self.node.graph_ctx;
        graph_ctx.document().read_only(graph_ctx.graph()).is_some()
    }

    /// Where this port stands among the node's inputs: the row it draws on.
    pub(crate) const fn idx(self) -> usize {
        self.idx
    }

    pub(crate) fn name(self) -> &'a str {
        &self.declared.name
    }

    /// Port tooltip from the func's declaration; empty when it declares none.
    pub(crate) fn description(self) -> &'a str {
        self.declared.description.as_deref().unwrap_or_default()
    }

    pub(crate) const fn data_type(self) -> DataType {
        self.declared.port.data_type
    }

    /// What the graph has bound here, or `None` for an unbound port.
    pub(crate) fn binding(self) -> Option<&'a Binding> {
        self.node.graph_ctx.body().wiring().binding(self.port())
    }

    /// The ghost this input holds in place of a binding: a wire or a constant the load kept
    /// and the input no longer takes, or whose producer is a ghost.
    pub(crate) fn ghost(self) -> Option<&'a GhostBinding> {
        self.node
            .graph_ctx
            .body()
            .ghost_wiring()
            .binding(self.port())
    }

    /// Required inputs render with more visual weight than optional ones.
    pub(crate) const fn required(self) -> bool {
        self.declared.port.required
    }

    /// Const-only inputs reject a wired binding: the wire gesture won't
    /// snap to them, so they can only hold a literal.
    pub(crate) const fn const_only(self) -> bool {
        self.declared.port.const_only
    }

    /// The constants the port admits past its type: a picker, a span, or none.
    pub(crate) fn domain(self) -> &'a ValueDomain {
        &self.declared.domain
    }

    /// The last run could not feed this port — its own verdict, so a port
    /// wired to a disabled or itself-unfed producer counts too, not just an
    /// unbound one. Only a `MissingInputs` status names ports, so this answers
    /// `false` under any other.
    pub(crate) fn missing(self) -> bool {
        self.node.missing_inputs().contains(&self.idx)
    }

    /// The literal this port falls back to when given a const binding: its
    /// declared default, else the zero value for its data type. `None` for a
    /// `Custom` type that declares none — there is no `ConstValue` of it, so
    /// the port can't be given an inline const.
    ///
    /// Owned: an enum's and an `Any`'s fallbacks are built here, not stored.
    pub(crate) fn default(self) -> Option<ConstValue> {
        self.node.graph_ctx.library().default_const(self.declared)
    }

    /// Whether the input can hold a constant: [`Self::default`] is some, answered without
    /// building it, for a reader that asks every frame.
    pub(crate) fn takes_const(self) -> bool {
        self.node.graph_ctx.library().takes_const(self.declared)
    }
}
