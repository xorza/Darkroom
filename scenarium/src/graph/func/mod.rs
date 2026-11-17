pub(crate) mod error;
pub(crate) mod event;
pub(crate) mod func_builder;
pub(crate) mod lambda;
mod macros;
pub(crate) mod signature;
pub(crate) mod value_variant;

use crate::graph::func::event::EventLambda;
use crate::graph::func::func_builder::FuncBuilder;
use crate::graph::func::lambda::FuncLambda;
use crate::graph::identity::{FuncId, FuncRef, PortId};
use crate::graph::node::CacheMode;
use crate::graph::ports::{InputDecl, Numbered, OutputDecl, Ports};

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

#[derive(Clone, Debug)]
pub struct FuncEvent {
    /// The number the func gives the event, as it numbers its ports.
    pub id: PortId,
    pub name: String,
    pub event_lambda: EventLambda,
}

/// A validated func declaration: its ports, behavior, implementation, and the `FuncRef` a compiled
/// node of it runs under, computed once by [`FuncBuilder::build`].
///
/// Read-only, so the signature always describes the ports beside it; to change a declaration,
/// take it back with [`into_builder`](Self::into_builder) and build it again.
#[derive(Clone, Debug)]
pub struct Func {
    func_ref: FuncRef,
    name: String,
    category: String,
    sink: bool,
    /// The [`CacheMode`] a new node of this func copies; existing nodes keep their own.
    default_cache_mode: CacheMode,
    behavior: FuncBehavior,
    description: Option<String>,
    ports: Ports,
    events: Vec<FuncEvent>,
    lambda: FuncLambda,
}

impl Func {
    /// Start a func declaration around its implementation. Defaults: `Impure`, non-sink, kept in
    /// RAM while memory allows ([`CacheMode::AUTO`]), with no category, ports or events.
    pub fn builder(id: FuncId, name: impl Into<String>, lambda: FuncLambda) -> FuncBuilder {
        FuncBuilder {
            id,
            name: name.into(),
            category: String::new(),
            sink: false,
            default_cache_mode: CacheMode::AUTO,
            behavior: FuncBehavior::Impure,
            description: None,
            ports: Ports::default(),
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
            ports: self.ports,
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

    pub const fn ports(&self) -> &Ports {
        &self.ports
    }

    pub fn inputs(&self) -> &[Numbered<InputDecl>] {
        &self.ports.inputs
    }

    pub fn outputs(&self) -> &[Numbered<OutputDecl>] {
        &self.ports.outputs
    }

    pub fn events(&self) -> &[FuncEvent] {
        &self.events
    }

    /// The event under `id`, if this func declares one.
    pub fn event(&self, id: PortId) -> Option<&FuncEvent> {
        self.events.iter().find(|event| event.id == id)
    }

    pub const fn lambda(&self) -> &FuncLambda {
        &self.lambda
    }

    /// What a compiled node of this func runs: its id, and the signature its state and cached
    /// values are keyed by.
    pub(crate) const fn func_ref(&self) -> FuncRef {
        self.func_ref
    }

    /// Whether this func recomputes every run: it has no content digest, so no cache mode is
    /// honored on a node of it.
    pub const fn impure(&self) -> bool {
        matches!(self.behavior, FuncBehavior::Impure)
    }
}

#[cfg(test)]
mod tests;
