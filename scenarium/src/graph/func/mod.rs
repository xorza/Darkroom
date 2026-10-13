pub(crate) mod error;
pub(crate) mod event;
pub(crate) mod func_input;
pub(crate) mod func_output;
pub(crate) mod lambda;
mod macros;
pub(crate) mod signature;
pub(crate) mod value_variant;

use crate::DataType;
use crate::data::data_type::Strictness;
use crate::graph::binding::Binding;
use crate::graph::binding_entry::BindingEntry;
use crate::graph::func::error::{FuncValidationError, OverrideRule};
use crate::graph::func::event::EventLambda;
use crate::graph::func::func_input::FuncInput;
use crate::graph::func::func_output::{FuncOutput, OutputType};
use crate::graph::func::lambda::FuncLambda;
use crate::graph::identity::FuncId;
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

#[derive(Clone, Debug)]
pub struct Func {
    pub id: FuncId,
    pub name: String,
    pub category: String,
    pub sink: bool,

    /// The [`CacheMode`] a freshly created node of this func copies into its
    /// `cache`. Defaults to [`CacheMode::None`] (no caching); raise it with the
    /// [`default_cache_mode`](Func::default_cache_mode) builder for funcs worth
    /// caching out of the box. Only a policy for *new* nodes — existing nodes
    /// keep whatever mode they were authored/saved with.
    pub default_cache_mode: CacheMode,

    pub behavior: FuncBehavior,

    pub description: Option<String>,
    pub inputs: Vec<FuncInput>,
    pub outputs: Vec<FuncOutput>,
    pub events: Vec<FuncEvent>,
    pub lambda: FuncLambda,
}

impl Func {
    /// Start a func definition around its implementation. Defaults: `Impure`,
    /// non-sink, cached nowhere, with no category, ports or events — set the
    /// rest with the chained builders below.
    pub fn new(id: FuncId, name: impl Into<String>, lambda: FuncLambda) -> Self {
        Self {
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

    #[must_use]
    pub fn category(mut self, category: impl Into<String>) -> Self {
        self.category = category.into();
        self
    }

    #[must_use]
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Mark the func `Pure` (same inputs → same outputs; cacheable).
    #[must_use]
    pub const fn pure(mut self) -> Self {
        self.behavior = FuncBehavior::Pure;
        self
    }

    #[must_use]
    pub const fn sink(mut self) -> Self {
        self.sink = true;
        self
    }

    /// Set the [`CacheMode`] that new nodes of this func adopt (see
    /// [`Func::default_cache_mode`]). Defaults to [`CacheMode::None`]; raise it
    /// for funcs whose output is worth caching by default.
    #[must_use]
    pub const fn default_cache_mode(mut self, mode: CacheMode) -> Self {
        self.default_cache_mode = mode;
        self
    }

    #[must_use]
    pub fn input(mut self, input: FuncInput) -> Self {
        self.inputs.push(input);
        self
    }

    #[must_use]
    pub fn inputs(mut self, inputs: impl IntoIterator<Item = FuncInput>) -> Self {
        self.inputs.extend(inputs);
        self
    }

    /// Add an output port. Build it with [`FuncOutput::new`], optionally chaining
    /// [`FuncOutput::description`] — mirrors the [`Func::input`] +
    /// [`FuncInput`] builder pattern.
    #[must_use]
    pub fn output(mut self, output: FuncOutput) -> Self {
        self.outputs.push(output);
        self
    }

    /// Add a *wildcard* output that mirrors input `mirrors_input`'s resolved
    /// type — a polymorphic passthrough / reroute port. See
    /// [`OutputType::Wildcard`].
    #[must_use]
    pub fn wildcard_output(mut self, name: impl Into<String>, mirrors_input: usize) -> Self {
        self.outputs.push(FuncOutput {
            name: name.into(),
            ty: OutputType::Wildcard {
                mirrors: mirrors_input,
            },
            description: None,
        });
        self
    }

    #[must_use]
    pub fn event(mut self, name: impl Into<String>, event_lambda: EventLambda) -> Self {
        self.events.push(FuncEvent {
            name: name.into(),
            event_lambda,
        });
        self
    }

    /// Whether this func recomputes every run — it has no content digest, so
    /// no cache mode is honored on a node instantiating it.
    ///
    /// A method rather than a field like [`sink`](Self::sink) because it
    /// reads off [`behavior`](Self::behavior), which names more than this one
    /// question.
    pub fn impure(&self) -> bool {
        self.behavior == FuncBehavior::Impure
    }

    /// The index of the input declared to override input `target`, if one is.
    /// [`validate`](Self::validate) holds a target to one overrider.
    pub fn overrider_of(&self, target: usize) -> Option<usize> {
        self.inputs
            .iter()
            .position(|input| input.overrides == Some(target))
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

    /// Validates this function declaration independently of a graph.
    pub fn validate(&self) -> Result<(), FuncValidationError> {
        if self.id.is_nil() {
            return Err(FuncValidationError::NilId);
        }
        if self.outputs.is_empty() && self.behavior != FuncBehavior::Impure {
            return Err(FuncValidationError::PureWithoutOutputs);
        }
        for (input_idx, input) in self.inputs.iter().enumerate() {
            if let DataType::Custom(type_id) | DataType::Enum(type_id) = &input.data_type
                && type_id.is_nil()
            {
                return Err(FuncValidationError::NilInputType {
                    func_id: self.id,
                    input_idx,
                });
            }
            // A declaration is checked without a registry — no `Library`
            // exists yet, and one may register this func before the enum types
            // it names. So an enum default need only *be* an enum literal
            // here; `Library::register_type` checks the name against the
            // variants once that type arrives, from whichever side is second.
            if let Some(default) = &input.default_value
                && !input.accepts_const(default, Strictness::Declared, |_, _| true)
            {
                return Err(FuncValidationError::InvalidDefault {
                    func_id: self.id,
                    input_idx,
                });
            }
        }
        for (input_idx, input) in self.inputs.iter().enumerate() {
            if let Some(target) = input.overrides
                && let Err(rule) = self.check_override(input_idx, target)
            {
                return Err(FuncValidationError::InvalidOverride {
                    func_id: self.id,
                    input_idx,
                    target,
                    rule,
                });
            }
        }
        for (output_idx, output) in self.outputs.iter().enumerate() {
            match &output.ty {
                OutputType::Fixed(DataType::Custom(type_id) | DataType::Enum(type_id)) => {
                    if type_id.is_nil() {
                        return Err(FuncValidationError::NilOutputType {
                            func_id: self.id,
                            output_idx,
                        });
                    }
                }
                OutputType::Wildcard { mirrors } => {
                    if *mirrors >= self.inputs.len() {
                        return Err(FuncValidationError::InvalidWildcardInput {
                            func_id: self.id,
                            output_idx,
                            input_idx: *mirrors,
                            input_count: self.inputs.len(),
                        });
                    }
                }
                OutputType::Fixed(_) => {}
            }
        }
        Ok(())
    }

    /// Whether input `input_idx` may override input `target`: an optional
    /// input over another, `const_only` one that nothing else overrides, and
    /// no chain — the override is one hop, resolved without a fixed point.
    fn check_override(&self, input_idx: usize, target: usize) -> Result<(), OverrideRule> {
        let Some(overridden) = self.inputs.get(target) else {
            return Err(OverrideRule::TargetOutOfRange);
        };
        if target == input_idx {
            return Err(OverrideRule::SelfTarget);
        }
        if self.inputs[input_idx].required {
            return Err(OverrideRule::RequiredOverride);
        }
        if !overridden.const_only {
            return Err(OverrideRule::WirableTarget);
        }
        if overridden.overrides.is_some() || self.overrider_of(input_idx).is_some() {
            return Err(OverrideRule::Chain);
        }
        if self.inputs[..input_idx]
            .iter()
            .any(|input| input.overrides == Some(target))
        {
            return Err(OverrideRule::SharedTarget);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
