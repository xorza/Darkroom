pub(crate) mod error;
pub(crate) mod event;
pub(crate) mod func_builder;
pub(crate) mod func_input;
pub(crate) mod func_output;
pub(crate) mod lambda;
mod macros;
pub(crate) mod signature;
pub(crate) mod value_variant;

use crate::graph::binding::Binding;
use crate::graph::binding_entry::BindingEntry;
use crate::graph::func::event::EventLambda;
use crate::graph::func::func_builder::FuncBuilder;
use crate::graph::func::func_input::FuncInput;
use crate::graph::func::func_output::FuncOutput;
use crate::graph::func::lambda::FuncLambda;
use crate::graph::identity::{FuncId, FuncRef};
use crate::graph::identity::{InputPort, NodeId};
use crate::graph::node::CacheMode;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum FuncBehavior {
    /// Could return different values for the same inputs.
    #[default]
    Impure,
    /// Always returns the same values for the same inputs.
    Pure,
}

#[derive(Clone, Debug)]
pub struct FuncEvent {
    pub name: String,
    pub event_lambda: EventLambda,
}

/// A validated func declaration: its ports, its behavior, its implementation,
/// and the [`FuncRef`] a node of it is authored against — its id with the
/// signature of its ports, computed once when [`FuncBuilder::build`] checked it.
///
/// Read-only, so the signature always describes the ports beside it. To change
/// a declaration, take it back with [`into_builder`](Self::into_builder) and
/// build it again.
#[derive(Clone, Debug)]
pub struct Func {
    func_ref: FuncRef,
    name: String,
    category: String,
    sink: bool,
    /// The [`CacheMode`] a freshly created node of this func copies into its
    /// `cache`. Only a policy for *new* nodes — existing nodes keep whatever
    /// mode they were authored/saved with.
    default_cache_mode: CacheMode,
    behavior: FuncBehavior,
    description: Option<String>,
    inputs: Vec<FuncInput>,
    outputs: Vec<FuncOutput>,
    events: Vec<FuncEvent>,
    lambda: FuncLambda,
}

impl Func {
    /// Start a func declaration around its implementation. Defaults: `Impure`,
    /// non-sink, cached nowhere, with no category, ports or events — set the
    /// rest with the builder's chained setters, then
    /// [`build`](FuncBuilder::build).
    pub fn builder(id: FuncId, name: impl Into<String>, lambda: FuncLambda) -> FuncBuilder {
        FuncBuilder {
            id,
            name: name.into(),
            category: String::new(),
            sink: false,
            default_cache_mode: CacheMode::None,
            behavior: FuncBehavior::Impure,
            description: None,
            inputs: Vec::new(),
            outputs: Vec::new(),
            events: Vec::new(),
            lambda,
        }
    }

    /// This declaration as a builder again, for an edit to build anew.
    pub fn into_builder(self) -> FuncBuilder {
        FuncBuilder {
            id: self.func_ref.id(),
            name: self.name,
            category: self.category,
            sink: self.sink,
            default_cache_mode: self.default_cache_mode,
            behavior: self.behavior,
            description: self.description,
            inputs: self.inputs,
            outputs: self.outputs,
            events: self.events,
            lambda: self.lambda,
        }
    }

    pub const fn id(&self) -> FuncId {
        self.func_ref.id()
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn category(&self) -> &str {
        &self.category
    }

    pub const fn sink(&self) -> bool {
        self.sink
    }

    pub const fn default_cache_mode(&self) -> CacheMode {
        self.default_cache_mode
    }

    pub const fn behavior(&self) -> FuncBehavior {
        self.behavior
    }

    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    pub fn inputs(&self) -> &[FuncInput] {
        &self.inputs
    }

    pub fn outputs(&self) -> &[FuncOutput] {
        &self.outputs
    }

    pub fn events(&self) -> &[FuncEvent] {
        &self.events
    }

    pub const fn lambda(&self) -> &FuncLambda {
        &self.lambda
    }

    /// The address a node of this func is authored against.
    pub(crate) const fn func_ref(&self) -> FuncRef {
        self.func_ref
    }

    /// Whether this func recomputes every run — it has no content digest, so
    /// no cache mode is honored on a node instantiating it.
    pub const fn impure(&self) -> bool {
        matches!(self.behavior, FuncBehavior::Impure)
    }

    /// The const bindings a fresh instance starts with: one per input that
    /// declares a default, at that input's port index.
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

#[cfg(test)]
mod tests;
