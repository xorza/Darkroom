//! The compiler's debug-only checks of a freshly linked artifact.
//!
//! These are *self-consistency* verdicts from the validation stage that follows the walk. Nothing
//! but a bug in this crate can raise one, so they surface only through the `is_debug()`-gated
//! wrapper — as values rather than panics, so a test can assert on which invariant broke.

use thiserror::Error;

use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;

use crate::exec::idx::{NodeIdx, OutputAddr};
use crate::graph::identity::FuncId;
use crate::graph::program_id::ProgramId;

/// Self-consistency checks for the compile artifact. Each fallible `validate` has an
/// `is_debug()`-gated `validate_debug` wrapper, so production call sites pay nothing
/// while tests can inspect exact validation errors.
#[derive(Debug, Error)]
pub(crate) enum CompiledGraphValidationError {
    #[error("execution node {program_id:?} has a nil func id")]
    NilFuncId { program_id: ProgramId },
    #[error("execution node {program_id:?} references missing func {func_id:?}")]
    MissingFunc {
        program_id: ProgramId,
        func_id: FuncId,
    },
    #[error("execution node {program_id:?} {pool} arity does not match its function")]
    Arity {
        program_id: ProgramId,
        pool: PortPool,
    },
    #[error("execution node {program_id:?} {pool} range is out of bounds")]
    Range {
        program_id: ProgramId,
        pool: PortPool,
    },
    #[error(
        "execution node {program_id:?} has an event subscriber outside the program: {subscriber:?}"
    )]
    MissingEventSubscriber {
        program_id: ProgramId,
        subscriber: NodeIdx,
    },
    #[error("execution node {program_id:?} binds to missing output {target:?}")]
    MissingBindingTarget {
        program_id: ProgramId,
        target: OutputAddr,
    },
    #[error("execution node {program_id:?} binds to out-of-range output {target:?}")]
    BindingOutputOutOfRange {
        program_id: ProgramId,
        target: OutputAddr,
    },
}

/// Which of a node's three packed port pools a fault names. The arity and range checks are the
/// same question asked of each pool, so the pool is a value the two variants carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PortPool {
    Input,
    Output,
    Event,
}

impl Display for PortPool {
    /// Lowercase, so it reads inside the sentences above.
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Input => "input",
            Self::Output => "output",
            Self::Event => "event",
        })
    }
}
