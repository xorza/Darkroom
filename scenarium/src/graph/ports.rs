//! [`Ports`]: the inputs and outputs a declaration shows and is wired by, each an [`InputDecl`]
//! or an [`OutputDecl`] under its [`PortId`].
//!
//! # Decisions
//!
//! - **A port's id is part of its declaration**, unique among the ports of its side. A func
//!   numbers its ports, and a number once shipped is never changed or used again, as a protobuf
//!   field's ("This number cannot be changed once your message type is in use", "Field numbers
//!   should never be reused", [field numbers](https://protobuf.dev/programming-guides/proto3/#assigning)).

use std::fmt;

use thiserror::Error;

use crate::data::const_value::ConstValue;
use crate::data::data_type::enum_type::EnumId;
use crate::data::data_type::{DataType, Strictness};
use crate::graph::binding::Binding;
use crate::graph::binding_entry::BindingEntry;
use crate::graph::func::value_variant::ValueVariant;
use crate::graph::identity::{InputPort, NodeId, PortId};

/// One input port a declaration shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputDecl {
    /// Nil until a [`FuncBuilder`](crate::FuncBuilder) numbers the port; a declaration refuses the
    /// nil id ([`Ports::check`]).
    pub id: PortId,
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
            id: PortId::nil(),
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
        enum_variant: impl FnOnce(EnumId, &str) -> bool,
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
    /// Nil until a [`FuncBuilder`](crate::FuncBuilder) numbers the port; a declaration refuses the
    /// nil id ([`Ports::check`]).
    pub id: PortId,
    pub name: String,
    pub data_type: DataType,
    /// One-line human explanation shown as the port's hover tooltip in the
    /// editor. Display-only — execution never reads it.
    pub description: Option<String>,
}

impl OutputDecl {
    pub fn new(name: impl Into<String>, data_type: DataType) -> Self {
        Self {
            id: PortId::nil(),
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
    /// Hold these ports to the rules every declaration passes: each id is set and unique among
    /// the ports of its side, each type names its registered entry under a set id, and each
    /// default fits its input. Exact kinds for a scalar default, as nobody chose a coercion here;
    /// an enum default need only be an enum literal, as the variants are the library's, which
    /// [`Library::check`](crate::Library::check) holds it to.
    pub fn check(&self) -> Result<(), PortsError> {
        check_ids(self.inputs.iter().map(|input| input.id), PortList::Inputs)?;
        check_ids(
            self.outputs.iter().map(|output| output.id),
            PortList::Outputs,
        )?;
        for (idx, input) in self.inputs.iter().enumerate() {
            if input.data_type.has_nil_id() {
                return Err(PortsError::NilType {
                    list: PortList::Inputs,
                    idx,
                });
            }
            if let Some(default) = &input.default_value
                && !input.accepts_const(default, Strictness::Declared, |_, _| true)
            {
                return Err(PortsError::InvalidDefault { idx });
            }
        }
        if let Some(idx) = self
            .outputs
            .iter()
            .position(|output| output.data_type.has_nil_id())
        {
            return Err(PortsError::NilType {
                list: PortList::Outputs,
                idx,
            });
        }
        Ok(())
    }

    /// The input under `id`, if these ports hold one.
    pub fn input(&self, id: PortId) -> Option<&InputDecl> {
        self.input_idx(id).map(|idx| &self.inputs[idx])
    }

    /// The output under `id`, if these ports hold one.
    pub fn output(&self, id: PortId) -> Option<&OutputDecl> {
        self.output_idx(id).map(|idx| &self.outputs[idx])
    }

    /// Where the input under `id` stands among the inputs: its row on a node, its slot in a
    /// compiled node.
    pub fn input_idx(&self, id: PortId) -> Option<usize> {
        self.inputs.iter().position(|input| input.id == id)
    }

    /// Where the output under `id` stands among the outputs; see [`Self::input_idx`].
    pub fn output_idx(&self, id: PortId) -> Option<usize> {
        self.outputs.iter().position(|output| output.id == id)
    }

    /// The const bindings a fresh node of these ports starts with: one per input that declares a
    /// default, on that input.
    pub fn default_bindings(&self, node_id: NodeId) -> impl Iterator<Item = BindingEntry> {
        self.inputs.iter().filter_map(move |input| {
            Some(BindingEntry {
                port: InputPort::new(node_id, input.id),
                binding: Binding::Const(input.default_value.clone()?),
            })
        })
    }
}

/// The id of the port a func numbers `number`: what
/// [`FuncBuilder::input`](crate::FuncBuilder::input) and its siblings give the port. A shipped
/// number is never changed or given to another port, as a saved node's wiring names the port by
/// it.
pub const fn numbered_port(number: u32) -> PortId {
    PortId::from_u128(number as u128)
}

/// Hold `ids`, the ports of one side in order, to the id rule: each set, and none twice.
pub(crate) fn check_ids(
    ids: impl IntoIterator<Item = PortId>,
    list: PortList,
) -> Result<(), PortsError> {
    let mut seen: Vec<PortId> = Vec::new();
    for (idx, port_id) in ids.into_iter().enumerate() {
        if port_id.is_nil() {
            return Err(PortsError::NilId { list, idx });
        }
        if seen.contains(&port_id) {
            return Err(PortsError::DuplicateId { list, idx, port_id });
        }
        seen.push(port_id);
    }
    Ok(())
}

/// Which side of a declaration a port lies on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortList {
    Inputs,
    Outputs,
    Events,
}

impl fmt::Display for PortList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Inputs => "input",
            Self::Outputs => "output",
            Self::Events => "event",
        })
    }
}

/// Why a list of ports is no declaration's: what [`Ports::check`] refuses.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum PortsError {
    #[error("{list} {idx} has the nil id")]
    NilId { list: PortList, idx: usize },
    #[error("{list} {idx} repeats the id {port_id:?} of an earlier one")]
    DuplicateId {
        list: PortList,
        idx: usize,
        port_id: PortId,
    },
    #[error("{list} {idx} names a type under the nil id")]
    NilType { list: PortList, idx: usize },
    #[error("input {idx} declares a default that matches neither its type nor its picker variants")]
    InvalidDefault { idx: usize },
}
