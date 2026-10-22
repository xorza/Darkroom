//! [`FuncBuilder`]: a func declaration in progress, checked and sealed into a
//! [`Func`] by [`build`](FuncBuilder::build).

use crate::data::data_type::{DataType, Strictness};
use crate::graph::func::error::FuncValidationError;
use crate::graph::func::event::EventLambda;
use crate::graph::func::func_input::FuncInput;
use crate::graph::func::func_output::FuncOutput;
use crate::graph::func::lambda::FuncLambda;
use crate::graph::func::signature::FuncSignature;
use crate::graph::func::{Func, FuncBehavior, FuncEvent};
use crate::graph::identity::{FuncId, FuncRef};
use crate::graph::node::CacheMode;

/// A func declaration in progress. Started by [`Func::builder`] or
/// [`Func::into_builder`], set through the chained setters or the fields, and
/// sealed by [`build`](Self::build).
#[derive(Clone, Debug)]
pub struct FuncBuilder {
    pub id: FuncId,
    pub name: String,
    pub category: String,
    pub sink: bool,
    pub default_cache_mode: CacheMode,
    pub behavior: FuncBehavior,
    pub description: Option<String>,
    pub inputs: Vec<FuncInput>,
    pub outputs: Vec<FuncOutput>,
    pub events: Vec<FuncEvent>,
    pub lambda: FuncLambda,
}

impl FuncBuilder {
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

    /// Set the [`CacheMode`] that new nodes of this func adopt. Defaults to
    /// [`CacheMode::None`]; raise it for funcs whose output is worth caching by
    /// default.
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
    /// [`FuncOutput::description`] — mirrors the [`input`](Self::input) +
    /// [`FuncInput`] builder pattern.
    #[must_use]
    pub fn output(mut self, output: FuncOutput) -> Self {
        self.outputs.push(output);
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

    /// Check the declaration and seal it, with the signature of its ports.
    ///
    /// # Panics
    /// If the declaration is invalid ([`validate`](Self::validate)): a func
    /// that could never be instantiated coherently is a bug in the code that
    /// declared it.
    pub fn build(self) -> Func {
        if let Err(error) = self.validate() {
            panic!("invalid function declaration {:?}: {error}", self.name);
        }
        let signature = FuncSignature::of(&self.inputs, &self.outputs, &self.events);
        Func {
            func_ref: FuncRef::new(self.id, signature),
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

    /// Check this declaration independently of a graph — what
    /// [`build`](Self::build) asserts.
    pub fn validate(&self) -> Result<(), FuncValidationError> {
        if self.id.is_nil() {
            return Err(FuncValidationError::NilId);
        }
        if self.outputs.is_empty() && self.behavior != FuncBehavior::Impure {
            return Err(FuncValidationError::PureWithoutOutputs { func_id: self.id });
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
        for (output_idx, output) in self.outputs.iter().enumerate() {
            match &output.data_type {
                DataType::Any => {
                    return Err(FuncValidationError::AnyOutput {
                        func_id: self.id,
                        output_idx,
                    });
                }
                DataType::Custom(type_id) | DataType::Enum(type_id) if type_id.is_nil() => {
                    return Err(FuncValidationError::NilOutputType {
                        func_id: self.id,
                        output_idx,
                    });
                }
                _ => {}
            }
        }
        Ok(())
    }
}
