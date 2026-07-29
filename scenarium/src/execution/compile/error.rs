//! What compiling rejects, and what a compiled artifact checks about itself.
//!
//! [`CompileError`] is the only one a caller sees: a graph that will not compile
//! against the library it names — a dropped func, a shrunk port list, a
//! type-mismatched binding — which a document can reach simply by being older
//! than the library. It is recoverable, and never enters the worker.
//!
//! The crate-private errors are *self-consistency* verdicts on an artifact
//! linking has already produced. Nothing but a bug in this crate can raise one,
//! so they surface only through the `is_debug()`-gated `validate_debug` wrappers
//! — as values rather than panics, so a test can assert on exactly which
//! invariant broke.

use thiserror::Error;

use crate::execution::identity::ExecutionNodeId;
use crate::execution::identity::{NodeIdx, OutputAddr};
use crate::graph::identity::FuncId;

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
    #[error(transparent)]
    Attribution(#[from] AttributionValidationError),
    #[error("execution node {e_node_id:?} has a nil func id")]
    NilFuncId { e_node_id: ExecutionNodeId },
    #[error("execution node {e_node_id:?} references missing func {func_id:?}")]
    MissingFunc {
        e_node_id: ExecutionNodeId,
        func_id: FuncId,
    },
    #[error("execution node {e_node_id:?} input arity does not match its function")]
    InputArity { e_node_id: ExecutionNodeId },
    #[error("execution node {e_node_id:?} output arity does not match its function")]
    OutputArity { e_node_id: ExecutionNodeId },
    #[error("execution node {e_node_id:?} event arity does not match its function")]
    EventArity { e_node_id: ExecutionNodeId },
    #[error("execution node {e_node_id:?} input range is out of bounds")]
    InputRange { e_node_id: ExecutionNodeId },
    #[error("execution node {e_node_id:?} output range is out of bounds")]
    OutputRange { e_node_id: ExecutionNodeId },
    #[error("execution node {e_node_id:?} event range is out of bounds")]
    EventRange { e_node_id: ExecutionNodeId },
    #[error(
        "execution node {e_node_id:?} has an event subscriber outside the program: {subscriber:?}"
    )]
    MissingEventSubscriber {
        e_node_id: ExecutionNodeId,
        subscriber: NodeIdx,
    },
    #[error("execution node {e_node_id:?} binds to missing output {target:?}")]
    MissingBindingTarget {
        e_node_id: ExecutionNodeId,
        target: OutputAddr,
    },
    #[error("execution node {e_node_id:?} binds to out-of-range output {target:?}")]
    BindingOutputOutOfRange {
        e_node_id: ExecutionNodeId,
        target: OutputAddr,
    },
}

#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum AttributionValidationError {
    #[error("attribution spans {len} nodes, not the program's {expected}")]
    LeafCount { len: usize, expected: usize },
    #[error("node {node_idx:?} is attributed to out-of-range scope {scope}")]
    LeafScope { node_idx: NodeIdx, scope: u32 },
    #[error("scope {scope} has parent {parent}, which does not precede it")]
    ScopeParent { scope: u32, parent: u32 },
}
