//! [`Func`]: a library declaration, the ports it shows and the lambda that runs it.
//!
//! # Decisions
//!
//! - **One field list for a func**, [`FuncDecl`], which a [`FuncBuilder`]'s setters write and a
//!   [`Func`] reads. Refused: the builder's fields public beside its setters, two routes to one
//!   field, with the list written out in each type.
//! - **A func's events are two columns in one order**, its declarations and their lambdas, pushed
//!   together by [`FuncBuilder::event`] and held to one length by the check. So a func lends its
//!   events as the `&[Numbered<EventDecl>]` an interface does, and a node's events read one way
//!   whatever it resolves to. Refused: one list of an event beside its lambda, which no slice of
//!   declarations can borrow from, so each reader went through a bridge enum.

pub(crate) mod error;
pub(crate) mod event;
pub(crate) mod func_builder;
pub(crate) mod func_decl;
pub(crate) mod lambda;
mod macros;
pub(crate) mod signature;

use crate::graph::func::event::EventLambda;
use crate::graph::func::func_builder::FuncBuilder;
use crate::graph::func::func_decl::FuncDecl;
use crate::graph::func::lambda::FuncLambda;
use crate::graph::func::signature::FuncSignature;
use crate::graph::identity::{FuncId, FuncRef, PortId};
use crate::graph::node::CacheMode;
use crate::graph::ports::{EventDecl, InputDecl, Numbered, OutputDecl, Ports};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum FuncBehavior {
    /// Could return different values for the same inputs.
    #[default]
    Impure,
    /// Always returns the same values for the same inputs. A sink with no outputs is pure when
    /// running it again on the same inputs changes nothing, as a display shows what it already
    /// shows: a run whose inputs fold to the digest it last ran under skips it, and so pulls
    /// nothing upstream for it, as TouchDesigner cooks a requested node only for a reason
    /// ([cook](https://derivative.ca/UserGuide/Cook)) and Nuke's viewer reuses what an op's hash
    /// already drew ([hashing](https://learn.foundry.com/nuke/developers/90/ndkdevguide/advanced/hashing.html)).
    Pure,
}

/// A validated func declaration: its ports, behavior, implementation, and the signature a
/// compiled node of it runs under, computed once by [`FuncBuilder::checked`].
///
/// Read-only, so the signature always describes the ports beside it.
#[derive(Clone, Debug)]
pub struct Func {
    decl: FuncDecl,
    signature: FuncSignature,
}

impl Func {
    /// Start a func declaration around its implementation. Defaults: `Impure`, non-sink, kept in
    /// RAM while memory allows ([`CacheMode::AUTO`]), with no category, ports or events.
    pub fn builder(id: FuncId, name: impl Into<String>, lambda: FuncLambda) -> FuncBuilder {
        FuncBuilder {
            decl: FuncDecl {
                id,
                name: name.into(),
                category: String::new(),
                sink: false,
                default_cache_mode: CacheMode::AUTO,
                behavior: FuncBehavior::Impure,
                description: None,
                ports: Ports::default(),
                events: Vec::new(),
                event_lambdas: Vec::new(),
                lambda,
            },
        }
    }

    pub const fn id(&self) -> FuncId {
        self.decl.id
    }

    pub fn name(&self) -> &str {
        &self.decl.name
    }

    pub fn category(&self) -> &str {
        &self.decl.category
    }

    pub const fn sink(&self) -> bool {
        self.decl.sink
    }

    /// The [`CacheMode`] a new node of this func copies; existing nodes keep their own.
    pub const fn default_cache_mode(&self) -> CacheMode {
        self.decl.default_cache_mode
    }

    pub const fn behavior(&self) -> FuncBehavior {
        self.decl.behavior
    }

    pub fn description(&self) -> Option<&str> {
        self.decl.description.as_deref()
    }

    pub const fn ports(&self) -> &Ports {
        &self.decl.ports
    }

    pub fn inputs(&self) -> &[Numbered<InputDecl>] {
        &self.decl.ports.inputs
    }

    pub fn outputs(&self) -> &[Numbered<OutputDecl>] {
        &self.decl.ports.outputs
    }

    pub fn events(&self) -> &[Numbered<EventDecl>] {
        &self.decl.events
    }

    /// The event under `id`, if this func declares one.
    pub fn event(&self, id: PortId) -> Option<&Numbered<EventDecl>> {
        self.decl.events.iter().find(|event| event.id == id)
    }

    /// What fires each event, in [`Self::events`]' order.
    pub fn event_lambdas(&self) -> &[EventLambda] {
        &self.decl.event_lambdas
    }

    pub const fn lambda(&self) -> &FuncLambda {
        &self.decl.lambda
    }

    /// What a compiled node of this func runs: its id, and the signature its state and cached
    /// values are keyed by.
    pub(crate) const fn func_ref(&self) -> FuncRef {
        FuncRef::new(self.decl.id, self.signature)
    }

    /// Whether this func recomputes every run: it has no content digest, so no cache mode is
    /// honored on a node of it.
    pub const fn impure(&self) -> bool {
        matches!(self.decl.behavior, FuncBehavior::Impure)
    }
}

#[cfg(feature = "internals")]
pub(crate) mod internals {
    use crate::graph::func::Func;
    use crate::graph::func::func_builder::FuncBuilder;

    impl Func {
        /// This declaration as a builder again, for a test to edit and build anew.
        pub fn into_builder(self) -> FuncBuilder {
            FuncBuilder { decl: self.decl }
        }
    }
}

#[cfg(test)]
mod tests;
