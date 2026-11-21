//! [`FuncBuilder`]: a func declaration in progress, checked and sealed into a
//! [`Func`] by [`checked`](FuncBuilder::checked).

use common::ErrorChain;

use crate::graph::func::error::FuncValidationError;
use crate::graph::func::event::EventLambda;
use crate::graph::func::func_decl::FuncDecl;
use crate::graph::func::signature::FuncSignature;
use crate::graph::func::{Func, FuncBehavior};
use crate::graph::identity::PortId;
use crate::graph::node::CacheMode;
use crate::graph::ports::{EventDecl, InputDecl, Numbered, OutputDecl};

/// A func declaration in progress, started by [`Func::builder`]. Its setters are the one way to
/// write a declaration; [`checked`](Self::checked) seals it.
#[derive(Clone, Debug)]
pub struct FuncBuilder {
    pub(super) decl: FuncDecl,
}

impl FuncBuilder {
    #[must_use]
    pub fn category(mut self, category: impl Into<String>) -> Self {
        self.decl.category = category.into();
        self
    }

    #[must_use]
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.decl.description = Some(description.into());
        self
    }

    /// Mark the func `Pure` (same inputs → same outputs; cacheable), or for a sink with no
    /// outputs, the same inputs → the same effect, skipped once done ([`FuncBehavior::Pure`]).
    #[must_use]
    pub const fn pure(mut self) -> Self {
        self.decl.behavior = FuncBehavior::Pure;
        self
    }

    #[must_use]
    pub const fn sink(mut self) -> Self {
        self.decl.sink = true;
        self
    }

    /// Set the [`CacheMode`] that new nodes of this func adopt; defaults to [`CacheMode::AUTO`].
    #[must_use]
    pub const fn default_cache_mode(mut self, mode: CacheMode) -> Self {
        self.decl.default_cache_mode = mode;
        self
    }

    /// Add an input port under the number `number`, which this func never gives another input
    /// and never changes once shipped: a saved node's wiring names the port by it.
    #[must_use]
    pub fn input(mut self, number: u32, input: InputDecl) -> Self {
        self.decl
            .ports
            .inputs
            .push(Numbered::new(PortId::new(number), input));
        self
    }

    /// Add an output port under the number `number`, as [`Self::input`] numbers an input.
    #[must_use]
    pub fn output(mut self, number: u32, output: OutputDecl) -> Self {
        self.decl
            .ports
            .outputs
            .push(Numbered::new(PortId::new(number), output));
        self
    }

    /// Add an event under the number `number`, as [`Self::input`] numbers an input, fired by
    /// `event_lambda`.
    #[must_use]
    pub fn event(mut self, number: u32, event: EventDecl, event_lambda: EventLambda) -> Self {
        self.decl
            .events
            .push(Numbered::new(PortId::new(number), event));
        self.decl.event_lambdas.push(event_lambda);
        self
    }

    /// Check the declaration and seal it, with the signature of its ports: the rules of
    /// [`FuncValidationError`], held apart from any graph.
    pub fn checked(self) -> Result<Func, FuncValidationError> {
        self.decl.check()?;
        let signature = FuncSignature::of(&self.decl.ports, &self.decl.events);
        Ok(Func {
            decl: self.decl,
            signature,
        })
    }

    /// [`Self::checked`], panicking on its error, which names the declaration.
    ///
    /// # Panics
    /// If the declaration is invalid: a func that could never be instantiated coherently is a bug
    /// in the code that declared it.
    pub fn build(self) -> Func {
        let name = self.decl.name.clone();
        self.checked().unwrap_or_else(|error| {
            panic!(
                "invalid function declaration {name:?}: {}",
                ErrorChain::new(&error)
            )
        })
    }
}

#[cfg(feature = "internals")]
pub(crate) mod internals {
    use crate::graph::func::func_builder::FuncBuilder;
    use crate::graph::func::func_decl::FuncDecl;

    impl FuncBuilder {
        /// The declaration in progress, for a fixture to change what no setter does.
        pub(crate) const fn decl_mut(&mut self) -> &mut FuncDecl {
            &mut self.decl
        }
    }
}
