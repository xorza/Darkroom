//! One input port of a node, with its declaration and current binding.

use scenarium::{Binding, ConstValue, DataType, FuncInput, InputPort, Library, ValueVariant};

use crate::gui::graph_ctx::node_ctx::NodeCtx;
use crate::gui::port_ref::PortRef;

/// One input port: what the func declares for it, and what the graph has
/// bound to it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct InputCtx<'a> {
    node: NodeCtx<'a>,
    port_idx: usize,
    declared: &'a FuncInput,
}

impl<'a> InputCtx<'a> {
    pub(super) const fn new(node: NodeCtx<'a>, port_idx: usize, declared: &'a FuncInput) -> Self {
        Self {
            node,
            port_idx,
            declared,
        }
    }

    /// This port's address in the graph.
    pub(crate) const fn port(self) -> InputPort {
        InputPort::new(self.node.node_id, self.port_idx)
    }

    /// This port's address in the canvas's glyph domains.
    pub(crate) const fn port_ref(self) -> PortRef {
        PortRef::input(self.node.node_id, self.port_idx)
    }

    pub(crate) fn name(self) -> &'a str {
        &self.declared.name
    }

    /// Port tooltip from the func's declaration; empty when it declares none.
    pub(crate) fn description(self) -> &'a str {
        self.declared.description.as_deref().unwrap_or_default()
    }

    pub(crate) const fn data_type(self) -> &'a DataType {
        &self.declared.data_type
    }

    /// What the graph has bound here, or `None` for an unbound port.
    pub(crate) fn binding(self) -> Option<&'a Binding> {
        self.node.graph_ctx.body().bindings.get(&self.port())
    }

    /// Required inputs render with more visual weight than optional ones.
    pub(crate) const fn required(self) -> bool {
        self.declared.required
    }

    /// Const-only inputs reject a wired binding: the wire gesture won't
    /// snap to them, so they can only hold a literal.
    pub(crate) const fn const_only(self) -> bool {
        self.declared.const_only
    }

    /// The editor picker's options for this port, e.g. named config presets.
    /// Empty is the common case.
    pub(crate) fn value_variants(self) -> &'a [ValueVariant] {
        &self.declared.value_variants
    }

    /// The last run could not feed this port — its own verdict, so a port
    /// wired to a disabled or itself-unfed producer counts too, not just an
    /// unbound one. Only a `MissingInputs` status names ports, so this answers
    /// `false` under any other.
    pub(crate) fn missing(self) -> bool {
        self.node.missing_inputs().contains(&self.port_idx)
    }

    /// The literal this port falls back to when given a const binding: its
    /// declared default, else the zero value for its data type. `None` for a
    /// `Custom` type that declares none — there is no `ConstValue` of it, so
    /// the port can't be given an inline const.
    ///
    /// Owned rather than borrowed: an enum's and an `Any`'s fallbacks are
    /// built here, not stored anywhere to point at.
    pub(crate) fn default(self) -> Option<ConstValue> {
        default_const_value(self.node.graph_ctx.library(), self.declared)
    }
}

/// See [`InputCtx::default`]. A free fn because the enum arm needs the
/// library rather than anything on the port.
fn default_const_value(library: &Library, input: &FuncInput) -> Option<ConstValue> {
    input.default_value.clone().or_else(|| {
        // An enum's first-variant default needs the library's registered variant
        // list — the bare `DataType::Enum(id)` doesn't carry it, so resolve it
        // here so an enum port gets the same const affordance as a scalar.
        match &input.data_type {
            DataType::Enum(id) => library
                .enum_variants(*id)
                .and_then(|variants| variants.first())
                .map(|first| ConstValue::Enum(first.clone())),
            // An untyped (`Any`) port has no concrete kind to seed; start it as
            // an empty string so the smart editor opens blank and infers the
            // kind from whatever the user types (see `const_editor::parse_any`).
            DataType::Any => Some(ConstValue::String(String::new())),
            data_type => data_type.default_value(),
        }
    })
}

#[cfg(test)]
mod tests {
    use scenarium::{ConstValue, DataType, FuncInput, Library, TypeEntry, TypeId};

    use crate::gui::graph_ctx::input_ctx::default_const_value;

    /// A declared default wins; an enum without one takes its first registered variant, and an
    /// unregistered one has none; `Any` starts blank; a scalar takes its type's zero; a custom
    /// type without a default has no const at all.
    #[test]
    fn an_input_defaults_to_its_declared_value_else_its_types() {
        let mut library = Library::default();
        let mode = TypeId::from_u128(1);
        library.register_type(
            mode,
            TypeEntry::enum_with_variants("Mode", vec!["Fast".into(), "Fine".into()]),
        );
        let input = |data_type: DataType| FuncInput::optional("x", data_type);
        let declared = FuncInput {
            default_value: Some(ConstValue::Int(5)),
            ..input(DataType::Int)
        };
        for (port, expected) in [
            (declared, Some(ConstValue::Int(5))),
            (
                input(DataType::Enum(mode)),
                Some(ConstValue::Enum("Fast".into())),
            ),
            (input(DataType::Enum(TypeId::from_u128(2))), None),
            (
                input(DataType::Any),
                Some(ConstValue::String(String::new())),
            ),
            (input(DataType::Int), Some(ConstValue::Int(0))),
            (input(DataType::Float), Some(ConstValue::Float(0.0))),
            (input(DataType::Custom(TypeId::from_u128(3))), None),
        ] {
            assert_eq!(
                default_const_value(&library, &port),
                expected,
                "{:?}",
                port.data_type
            );
        }
    }
}
