//! Test fixtures, available only to in-tree tests and the downstream
//! `internals` dev feature.
//!
//! [`graph`] is the harness: a graph and its library built together and
//! addressed by name, which is what a fixture reaches for. [`calls`] is the
//! counter its counted bodies are built on.
//!
//! What is left in this file is [`with_stub_lambda`], the one thing the
//! harness deliberately cannot supply: [`NodeSpec`](graph::NodeSpec) names
//! ports `in0`/`out0` by position, so a test about how an editor *renders* a
//! port has to state its own [`Func`] — and `Library::add` rejects one with no
//! implementation.

pub mod calls;
#[cfg(test)]
pub(crate) mod engine;
#[cfg(test)]
pub(crate) mod func_invoker;
pub mod graph;
#[cfg(test)]
pub(crate) mod program;
#[cfg(test)]
pub(crate) mod worker;

use crate::async_lambda;
use crate::graph::func::Func;

/// `func` with a body that does nothing, so a library will hold it.
pub fn with_stub_lambda(func: Func) -> Func {
    func.lambda(async_lambda!(|_| { Ok(()) }))
}
