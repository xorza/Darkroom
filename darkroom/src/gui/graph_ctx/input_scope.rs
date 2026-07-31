//! One input port of a node, with its declaration and current binding.

use scenarium::{Binding, DataType, FuncInput, InputPort, Library, StaticValue, ValueVariant};

use crate::core::document::PortRef;
use crate::gui::graph_ctx::node_scope::NodeScope;

/// One input port: what the func declares for it, and what the graph has
/// bound to it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct InputScope<'a> {
    node: NodeScope<'a>,
    port_idx: usize,
    declared: &'a FuncInput,
}

impl<'a> InputScope<'a> {
    pub(super) fn new(node: NodeScope<'a>, port_idx: usize, declared: &'a FuncInput) -> Self {
        Self {
            node,
            port_idx,
            declared,
        }
    }

    /// This port's address in the graph.
    pub(crate) fn port(self) -> InputPort {
        InputPort::new(self.node.id, self.port_idx)
    }

    /// This port's address in the canvas's glyph domains.
    pub(crate) fn port_ref(self) -> PortRef {
        PortRef::input(self.node.id, self.port_idx)
    }

    pub(crate) fn name(self) -> &'a str {
        &self.declared.name
    }

    /// Port tooltip from the func's declaration; empty when it declares none.
    pub(crate) fn description(self) -> &'a str {
        self.declared.description.as_deref().unwrap_or_default()
    }

    pub(crate) fn ty(self) -> &'a DataType {
        &self.declared.data_type
    }

    /// What the graph has bound here, or `None` for an unbound port.
    pub(crate) fn binding(self) -> Option<&'a Binding> {
        self.node.graph_ctx.body().bindings.get(&self.port())
    }

    /// Required inputs render with more visual weight than optional ones.
    pub(crate) fn required(self) -> bool {
        self.declared.required
    }

    /// Const-only inputs reject a wired binding: the connection gesture won't
    /// snap to them, so they can only hold a literal.
    pub(crate) fn const_only(self) -> bool {
        self.declared.const_only
    }

    /// The editor picker's options for this port, e.g. named config presets.
    /// Empty is the common case.
    pub(crate) fn value_variants(self) -> &'a [ValueVariant] {
        &self.declared.value_variants
    }

    /// The last run could not feed this port — its own verdict, so a port
    /// wired to a disabled or itself-unfed producer counts too, not just an
    /// unbound one. Renders highlighted while the node reads `MissingInputs`.
    pub(crate) fn missing(self) -> bool {
        self.node.missing_inputs().contains(&self.port_idx)
    }

    /// The literal this port falls back to when given a const binding: its
    /// declared default, else the zero value for its data type. `None` for a
    /// `Custom` type — there is no `StaticValue` for it, so the port can't be
    /// given an inline const.
    ///
    /// Owned rather than borrowed: an enum's and an `Any`'s fallbacks are
    /// built here, not stored anywhere to point at.
    pub(crate) fn default(self) -> Option<StaticValue> {
        default_static_value(self.node.graph_ctx.library(), self.declared)
    }
}

/// See [`InputScope::default`]. A free fn because the enum arm needs the
/// library rather than anything on the port.
fn default_static_value(library: &Library, input: &FuncInput) -> Option<StaticValue> {
    input.default_value.clone().or_else(|| {
        // An enum's first-variant default needs the library's registered variant
        // list — the bare `DataType::Enum(id)` doesn't carry it, so resolve it
        // here so an enum port gets the same const affordance as a scalar.
        match &input.data_type {
            DataType::Enum(id) => library
                .enum_variants(*id)
                .and_then(|variants| variants.first())
                .map(|first| StaticValue::Enum(first.clone())),
            // An untyped (`Any`) port has no concrete kind to seed; start it as
            // an empty string so the smart editor opens blank and infers the
            // kind from whatever the user types (see `value_editor::parse_any`).
            DataType::Any => Some(StaticValue::String(String::new())),
            ty => ty.default_value(),
        }
    })
}
