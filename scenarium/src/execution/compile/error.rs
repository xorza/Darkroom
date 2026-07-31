//! What compiling rejects, including the compiler's debug-only checks of a
//! freshly linked artifact.
//!
//! [`CompileError`] is the only one a caller sees: a graph that will not compile
//! against the library it names — a dropped func, a shrunk port list, a
//! type-mismatched binding — which a document can reach simply by being older
//! than the library. It is recoverable, and never enters the worker.
//!
//! The crate-private errors are *self-consistency* verdicts from the validation
//! stage that follows the walk. Nothing but a bug in this crate can raise one, so they
//! surface only through its `is_debug()`-gated wrapper — as values rather than
//! panics, so a test can assert on exactly which invariant broke.

use thiserror::Error;

use crate::execution::identity::{NodeIdx, OutputAddr};
use crate::graph::identity::{FuncId, NodeId};

/// The graph won't compile against the library: a document can be stale
/// against an evolved library (a dropped func, a shrunk port list, a
/// type-mismatched binding), so this is a recoverable error the caller
/// surfaces, not a logic bug. The compile-phase counterpart of the run-phase
/// [`Error`](crate::execution::error::Error) — the two can't be confused at the type
/// level, and only `compile` produces it.
#[derive(Debug, Error)]
#[error("invalid graph: {message}")]
pub struct CompileError {
    pub message: String,
}

/// Self-consistency checks for the compile artifact. Each fallible `validate` has an
/// `is_debug()`-gated `validate_debug` wrapper, so production call sites pay nothing
/// while tests can inspect exact validation errors.
#[derive(Debug, Error)]
pub(crate) enum CompiledGraphValidationError {
    #[error("execution node {node_id:?} has a nil func id")]
    NilFuncId { node_id: NodeId },
    #[error("execution node {node_id:?} references missing func {func_id:?}")]
    MissingFunc { node_id: NodeId, func_id: FuncId },
    #[error("execution node {node_id:?} {pool} arity does not match its function")]
    Arity { node_id: NodeId, pool: PortPool },
    #[error("execution node {node_id:?} {pool} range is out of bounds")]
    Range { node_id: NodeId, pool: PortPool },
    #[error(
        "execution node {node_id:?} has an event subscriber outside the program: {subscriber:?}"
    )]
    MissingEventSubscriber {
        node_id: NodeId,
        subscriber: NodeIdx,
    },
    #[error("execution node {node_id:?} binds to missing output {target:?}")]
    MissingBindingTarget { node_id: NodeId, target: OutputAddr },
    #[error("execution node {node_id:?} binds to out-of-range output {target:?}")]
    BindingOutputOutOfRange { node_id: NodeId, target: OutputAddr },
}

/// Which of a node's three packed port pools a fault names.
///
/// The arity and range checks are the same question asked of each pool, so the
/// pool is a value the two variants carry rather than three variants apiece.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PortPool {
    Input,
    Output,
    Event,
}

impl std::fmt::Display for PortPool {
    /// Lowercase, so it reads inside the sentences above.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            PortPool::Input => "input",
            PortPool::Output => "output",
            PortPool::Event => "event",
        })
    }
}
