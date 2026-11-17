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
        self.node.graph_ctx.body().bindings.get(&self.port())
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
        default_const_value(self.node.graph_ctx.library(), self.declared)
    }

    /// Whether the input can hold a constant: [`Self::default`] is some, answered without
    /// building it, for a reader that asks every frame.
    pub(crate) fn takes_const(self) -> bool {
        takes_const(self.node.graph_ctx.library(), self.declared)
    }
}

/// See [`InputCtx::takes_const`]: the rule [`default_const_value`] follows, read off the port
/// and the library alone.
fn takes_const(library: &Library, input: &InputDecl) -> bool {
    input.default_value.is_some()
        || match input.data_type {
            DataType::Enum(id) => library
                .enum_type(id)
                .is_some_and(|enum_type| !enum_type.variants().is_empty()),
            DataType::Any => true,
            data_type => data_type.default_value().is_some(),
        }
}

/// See [`InputCtx::default`]. A free fn because the enum arm needs the
/// library rather than anything on the port.
fn default_const_value(library: &Library, input: &InputDecl) -> Option<ConstValue> {
    input.default_value.clone().or_else(|| {
        // `DataType::Enum(id)` doesn't carry its variants; the library's registry does.
        match input.data_type {
            DataType::Enum(id) => library
                .enum_type(id)
                .and_then(|enum_type| enum_type.variants().first())
                .map(|first| ConstValue::Enum(first.clone())),
            // An `Any` port has no kind to seed: an empty string opens the editor blank, and
            // it infers the kind from the text typed (see `const_editor::parse_any`).
            DataType::Any => Some(ConstValue::String(String::new())),
            data_type => data_type.default_value(),
        }
    })
}

#[cfg(test)]
mod tests {
    use scenarium::{ConstValue, CustomTypeId, DataType, EnumId, EnumType, InputDecl, Library};

    use crate::gui::graph_ctx::input_ctx::{default_const_value, takes_const};

    /// A declared default wins; an enum without one takes its first registered variant, and an
    /// unregistered one has none; `Any` starts blank; a scalar takes its type's zero; a custom
    /// type without a default has no const at all.
    #[test]
    fn an_input_defaults_to_its_declared_value_else_its_types() {
        let mut library = Library::default();
        let mode = EnumId::from_u128(1);
        library.register_enum(
            mode,
            EnumType::new("Mode", vec!["Fast".into(), "Fine".into()]),
        );
        let input = |data_type: DataType| InputDecl::optional("x", data_type);
        let declared = InputDecl {
            default_value: Some(ConstValue::Int(5)),
            ..input(DataType::Int)
        };
        for (port, expected) in [
            (declared, Some(ConstValue::Int(5))),
            (
                input(DataType::Enum(mode)),
                Some(ConstValue::Enum("Fast".into())),
            ),
            (input(DataType::Enum(EnumId::from_u128(2))), None),
            (
                input(DataType::Any),
                Some(ConstValue::String(String::new())),
            ),
            (input(DataType::Int), Some(ConstValue::Int(0))),
            (input(DataType::Float), Some(ConstValue::Float(0.0))),
            (input(DataType::Custom(CustomTypeId::from_u128(3))), None),
        ] {
            assert_eq!(
                takes_const(&library, &port),
                expected.is_some(),
                "{:?} takes a const exactly when it has a default",
                port.data_type
            );
            assert_eq!(
                default_const_value(&library, &port),
                expected,
                "{:?}",
                port.data_type
            );
        }
    }
}
