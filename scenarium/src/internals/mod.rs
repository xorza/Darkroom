//! Test fixtures, available only to in-tree tests and the downstream
//! `internals` dev feature.
//!
//! [`graph`] is the harness: a graph and its library built together and
//! addressed by name, which is what a fixture reaches for. [`calls`] is the
//! counter its counted bodies are built on.
//!
//! What is left in this file is [`stub_func`] and [`stub_event`], the
//! do-nothing bodies the harness deliberately cannot supply:
//! [`NodeSpec`](graph::NodeSpec) names ports `in0`/`out0` by position, so a
//! test about how an editor *renders* a port has to state its own [`Func`] —
//! and a `Func` cannot be built without an implementation.

#[cfg(test)]
pub(crate) mod blob;
pub mod calls;
#[cfg(test)]
pub(crate) mod engine;
pub mod func_invoker;
pub mod graph;
#[cfg(test)]
pub(crate) mod program;
#[cfg(test)]
pub(crate) mod worker;

use crate::graph::func::Func;
use crate::graph::func::event::EventLambda;
use crate::graph::func::lambda::FuncLambda;
use crate::graph::identity::FuncId;

/// A func whose body does nothing, for a fixture that never runs it.
pub fn stub_func(id: FuncId, name: impl Into<String>) -> Func {
    Func::new(id, name, FuncLambda::stub())
}

/// An event body that does nothing, for a fixture that never fires it.
pub fn stub_event() -> EventLambda {
    EventLambda::new(|_| Box::pin(async {}))
}
