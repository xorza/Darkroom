//! [`Ports`]: the inputs and outputs a declaration shows and is wired by, each an [`InputDecl`]
//! or an [`OutputDecl`].

use crate::data::const_value::ConstValue;
use crate::data::data_type::{DataType, Strictness, TypeId};
use crate::graph::binding::Binding;
use crate::graph::binding_entry::BindingEntry;
use crate::graph::func::value_variant::ValueVariant;
use crate::graph::identity::{InputPort, NodeId};

/// One input port a declaration shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputDecl {
    pub name: String,
    pub required: bool,
    pub data_type: DataType,
    /// Hover-tooltip text in the editor (units, range, meaning); execution never reads it.
    pub description: Option<String>,
    /// When set, this input may only hold a `Const`: a `Wire` into it is rejected by graph
    /// validation. For inputs a node reads as configuration.
    pub const_only: bool,
    pub default_value: Option<ConstValue>,
    pub value_variants: Vec<ValueVariant>,
}

impl InputDecl {
    /// A required input of `data_type` (no const default).
    pub fn required(name: impl Into<String>, data_type: DataType) -> Self {
        Self {
            name: name.into(),
            required: true,
            data_type,
            description: None,
            const_only: false,
            default_value: None,
            value_variants: Vec::new(),
        }
    }

    /// An optional input of `data_type`; chain [`Self::default`] to seed a const default value.
    pub fn optional(name: impl Into<String>, data_type: DataType) -> Self {
        Self {
            required: false,
            ..Self::required(name, data_type)
        }
    }

    /// Seed this input's const default value.
    #[must_use]
    pub fn default(mut self, value: impl Into<ConstValue>) -> Self {
        self.default_value = Some(value.into());
        self
    }

    /// Attach the port's hover-tooltip text.
    #[must_use]
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Restrict this input to a `Const`; see [`InputDecl::const_only`].
    #[must_use]
    pub const fn const_only(mut self) -> Self {
        self.const_only = true;
        self
    }

    /// Attach the editor picker variants (`ValueVariant`s).
    #[must_use]
    pub fn variants(mut self, variants: Vec<ValueVariant>) -> Self {
        self.value_variants = variants;
        self
    }

    /// Whether `value` may sit on this input as a `Const`: the whole const gate, at declaration
    /// and at compile time. A port contributes two things ahead of the type table in
    /// [`DataType::accepts_const`]:
    ///
    /// - a **picker** port (one carrying `value_variants`) takes exactly one of the values it
    ///   offers, whatever its declared type says;
    /// - `Null` is "explicitly unset", so it fits an optional port and no required one (lens's
    ///   config machinery authors it on `Option`-field inputs and reads it back as `None`).
    ///
    /// `strictness` and `enum_variant` are the caller's; see [`Strictness`].
    pub(crate) fn accepts_const(
        &self,
        value: &ConstValue,
        strictness: Strictness,
        enum_variant: impl FnOnce(TypeId, &str) -> bool,
    ) -> bool {
        if !self.value_variants.is_empty() {
            return self.value_variants.iter().any(|v| v.value == *value);
        }
        if matches!(value, ConstValue::Null) {
            return !self.required;
        }
        self.data_type
            .accepts_const(value, strictness, enum_variant)
    }
}

/// One output port a declaration shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputDecl {
    pub name: String,
    pub data_type: DataType,
    /// One-line human explanation shown as the port's hover tooltip in the
    /// editor. Display-only — execution never reads it.
    pub description: Option<String>,
}

impl OutputDecl {
    pub fn new(name: impl Into<String>, data_type: DataType) -> Self {
        Self {
            name: name.into(),
            data_type,
            description: None,
        }
    }

    /// Attach the port's hover-tooltip text. See [`OutputDecl::description`].
    #[must_use]
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}

/// The inputs and outputs a declaration shows, each in the order a node draws them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ports {
    pub inputs: Vec<InputDecl>,
    pub outputs: Vec<OutputDecl>,
}

impl Ports {
    /// The const bindings a fresh node of these ports starts with: one per input that declares a
    /// default, at that input's port index.
    pub fn default_bindings(&self, node_id: NodeId) -> impl Iterator<Item = BindingEntry> {
        self.inputs
            .iter()
            .enumerate()
            .filter_map(move |(port_idx, input)| {
                Some(BindingEntry {
                    port: InputPort::new(node_id, port_idx),
                    binding: Binding::Const(input.default_value.clone()?),
                })
            })
    }
}
