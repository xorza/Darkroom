//! [`FuncBuilder`]: a func declaration in progress, checked and sealed into a
//! [`Func`] by [`build`](FuncBuilder::build).

use common::ErrorChain;

use crate::data::data_type::DataType;
use crate::graph::func::error::FuncValidationError;
use crate::graph::func::event::EventLambda;
use crate::graph::func::lambda::FuncLambda;
use crate::graph::func::signature::FuncSignature;
use crate::graph::func::{Func, FuncBehavior, FuncEvent};
use crate::graph::identity::{FuncId, FuncRef, PortId};
use crate::graph::node::CacheMode;
use crate::graph::ports::{InputDecl, Numbered, OutputDecl, PortList, Ports, check_ids};

/// A func declaration in progress, started by [`Func::builder`] or [`Func::into_builder`].
#[derive(Clone, Debug)]
pub struct FuncBuilder {
    pub id: FuncId,
    pub name: String,
    pub category: String,
    pub sink: bool,
    pub default_cache_mode: CacheMode,
    pub behavior: FuncBehavior,
    pub description: Option<String>,
    pub ports: Ports,
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

    /// Set the [`CacheMode`] that new nodes of this func adopt; defaults to [`CacheMode::None`].
    #[must_use]
    pub const fn default_cache_mode(mut self, mode: CacheMode) -> Self {
        self.default_cache_mode = mode;
        self
    }

    /// Add an input port under the number `number`, which this func never gives another input
    /// and never changes once shipped: a saved node's wiring names the port by it.
    #[must_use]
    pub fn input(mut self, number: u32, input: InputDecl) -> Self {
        self.ports
            .inputs
            .push(Numbered::new(PortId::new(number), input));
        self
    }

    /// Add an output port under the number `number`, as [`Self::input`] numbers an input.
    #[must_use]
    pub fn output(mut self, number: u32, output: OutputDecl) -> Self {
        self.ports
            .outputs
            .push(Numbered::new(PortId::new(number), output));
        self
    }

    /// Add an event under the number `number`, as [`Self::input`] numbers an input.
    #[must_use]
    pub fn event(
        mut self,
        number: u32,
        name: impl Into<String>,
        event_lambda: EventLambda,
    ) -> Self {
        self.events.push(FuncEvent {
            id: PortId::new(number),
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
            panic!(
                "invalid function declaration {:?}: {}",
                self.name,
                ErrorChain::new(&error)
            );
        }
        let signature = FuncSignature::of(&self.ports, &self.events);
        Func {
            func_ref: FuncRef::new(self.id, signature),
            name: self.name,
            category: self.category,
            sink: self.sink,
            default_cache_mode: self.default_cache_mode,
            behavior: self.behavior,
            description: self.description,
            ports: self.ports,
            events: self.events,
            lambda: self.lambda,
        }
    }

    /// Check this declaration independently of a graph — what
    /// [`build`](Self::build) asserts: the rules every declaration's ports pass
    /// ([`Ports::check`]), and a func's own, an id, an output for a pure func, no `Any` output
    /// and each event's number.
    pub fn validate(&self) -> Result<(), FuncValidationError> {
        if self.id.is_nil() {
            return Err(FuncValidationError::NilId);
        }
        let func_id = self.id;
        if self.ports.outputs.is_empty() && self.behavior != FuncBehavior::Impure {
            return Err(FuncValidationError::PureWithoutOutputs { func_id });
        }
        self.ports
            .check()
            .and_then(|()| check_ids(self.events.iter().map(|event| event.id), PortList::Events))
            .map_err(|error| FuncValidationError::Ports { func_id, error })?;
        if let Some(output_idx) = self
            .ports
            .outputs
            .iter()
            .position(|output| output.data_type == DataType::Any)
        {
            return Err(FuncValidationError::AnyOutput {
                func_id,
                output_idx,
            });
        }
        Ok(())
    }
}
