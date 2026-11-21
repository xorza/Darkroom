//! [`Ports`]: the inputs and outputs a declaration shows and is wired by, each an [`InputDecl`]
//! or an [`OutputDecl`] under its [`PortId`], a [`Numbered`].
//!
//! A func's ports and a compound's interface are both a `Ports`, held to one rule
//! ([`Ports::check`]), so a port exposed on an interface keeps what made it: a const-only input,
//! a picker, an enum.
//!
//! # Decisions
//!
//! - **A port's id is part of its declaration**, unique among the ports of its side. A func
//!   numbers its ports, and a number once shipped is never changed or used again, as a protobuf
//!   field's ("This number cannot be changed once your message type is in use", "Field numbers
//!   should never be reused", [field numbers](https://protobuf.dev/programming-guides/proto3/#assigning)).
//!   An interface mints a random id for each port ([`PortId::mint`]), so two copies of a
//!   definition changed apart give one id to two ports only by the odds [`PortId`] states.
//! - **A declaration does not hold its own id.** A list of ports holds each under its number, as
//!   [`NodeRef`](crate::NodeRef) pairs a node with the id it was found under, and
//!   [`FuncBuilder::input`](crate::FuncBuilder::input) and an interface are the two places that
//!   pair them. Refused: an `id` field that is nil until the port is numbered, a state a check
//!   had to catch.
//! - **A [`Numbered`] port reads as its port**, through `Deref`, at every site. A departure from
//!   [C-DEREF](https://rust-lang.github.io/api-guidelines/predictability.html), "only smart
//!   pointers implement `Deref`", for a pair that is its port with a number beside it. Refused:
//!   `.port.` at each read.

use std::ops::{Deref, DerefMut};

use serde::{Deserialize, Serialize};

use crate::data::const_value::ConstValue;
use crate::data::data_type::enum_type::EnumId;
use crate::data::data_type::{DataType, Strictness};
use crate::decl::ports::error::{PortList, PortsError};
use crate::decl::value_domain::ValueDomain;
use crate::decl::value_domain::float_span::FloatSpan;
use crate::decl::value_domain::int_span::IntSpan;
use crate::decl::value_domain::value_variant::ValueVariant;
use crate::identity::PortId;

pub(crate) mod error;

/// A port's declaration under its id: the item a declaration's lists of ports hold.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Numbered<T> {
    pub id: PortId,
    pub port: T,
}

impl<T> Numbered<T> {
    pub const fn new(id: PortId, port: T) -> Self {
        Self { id, port }
    }
}

impl<T> Deref for Numbered<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.port
    }
}

impl<T> DerefMut for Numbered<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.port
    }
}

/// One input port a declaration shows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputDecl {
    pub name: String,
    pub required: bool,
    pub data_type: DataType,
    /// Hover-tooltip text in the editor (units, range, meaning); execution never reads it.
    pub description: Option<String>,
    /// When set, this input may only hold a `Const`, or a wire from an output that holds only a
    /// `Const` itself: the `Inputs` node's mirror of a const-only interface input. For inputs a
    /// node reads as configuration.
    pub const_only: bool,
    pub default_value: Option<ConstValue>,
    /// The constants the input admits past what its type holds.
    pub domain: ValueDomain,
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
            domain: ValueDomain::Any,
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

    /// The constants the input admits; see [`ValueDomain`].
    #[must_use]
    pub fn domain(mut self, domain: ValueDomain) -> Self {
        self.domain = domain;
        self
    }

    /// [`Self::domain`] of a picker of `variants`.
    #[must_use]
    pub fn variants(self, variants: Vec<ValueVariant>) -> Self {
        self.domain(ValueDomain::Variants(variants))
    }

    /// [`Self::domain`] of an integer `span`.
    #[must_use]
    pub fn int_span(self, span: IntSpan) -> Self {
        self.domain(ValueDomain::Int(span))
    }

    /// [`Self::domain`] of a float `span`.
    #[must_use]
    pub fn float_span(self, span: FloatSpan) -> Self {
        self.domain(ValueDomain::Float(span))
    }

    /// Whether `value` may sit on this input as a `Const`: the whole const gate, at declaration
    /// and at compile time. A port contributes two things ahead of the type table in
    /// [`DataType::accepts_const`]:
    ///
    /// - its **domain**: a picker takes exactly one of the values it offers, whatever its
    ///   declared type says, and a span refuses a number outside it, the number read as the func
    ///   reads it;
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
        match (&self.domain, self.domain.admits(value)) {
            (ValueDomain::Variants(_), Some(admitted)) => return admitted,
            (_, Some(false)) => return false,
            _ => {}
        }
        if matches!(value, ConstValue::Null) {
            return !self.required;
        }
        self.data_type
            .accepts_const(value, strictness, enum_variant)
    }
}

/// One output port a declaration shows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

/// An event an interface fires or its trigger: a name and what it is for, as a func's
/// event declares beside its lambda.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventDecl {
    pub name: String,
    pub description: Option<String>,
}

impl EventDecl {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: None,
        }
    }

    /// Attach what the event is for, as [`OutputDecl::description`] does for an output.
    #[must_use]
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}

/// The inputs and outputs a declaration shows, each in the order a node draws them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ports {
    pub inputs: Vec<Numbered<InputDecl>>,
    pub outputs: Vec<Numbered<OutputDecl>>,
}

impl Ports {
    /// Hold these ports to the rules every declaration passes: each id is unique among the ports
    /// of its side, each type names its registered entry under a set id, each domain fits its
    /// input's type, and each default fits its input. Exact kinds for a scalar default, as nobody
    /// chose a coercion here; an enum default need only be an enum literal, as the variants are
    /// the library's, which [`LibraryBuilder::build`](crate::LibraryBuilder::build) holds it to.
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
            if !input.domain.fits(input.data_type) {
                return Err(PortsError::DomainMismatch { idx });
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
    pub fn input(&self, id: PortId) -> Option<&Numbered<InputDecl>> {
        self.input_idx(id).map(|idx| &self.inputs[idx])
    }

    /// The output under `id`, if these ports hold one.
    pub fn output(&self, id: PortId) -> Option<&Numbered<OutputDecl>> {
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
}

/// Hold `ids`, the ports of one side in order, to the id rule: none twice.
pub(crate) fn check_ids(
    ids: impl IntoIterator<Item = PortId>,
    list: PortList,
) -> Result<(), PortsError> {
    let mut seen: Vec<PortId> = Vec::new();
    for (idx, port_id) in ids.into_iter().enumerate() {
        if seen.contains(&port_id) {
            return Err(PortsError::DuplicateId { list, idx, port_id });
        }
        seen.push(port_id);
    }
    Ok(())
}
