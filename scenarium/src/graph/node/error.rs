//! What a node's declaration and its invocation reject.
//!
//! [`FuncValidationError`] is a *registration* failure — a `Func` that could
//! never be instantiated coherently — caught when a library is assembled, long
//! before any graph references it. [`InvokeError`] is the one a lambda hands
//! back at run time; the executor attributes it to the node and keeps going,
//! rather than taking the run down.

use thiserror::Error;

use std::error;
use std::fmt;

use crate::graph::identity::FuncId;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FuncValidationError {
    #[error("function id must not be nil")]
    NilId,
    #[error("Function with no outputs should be impure")]
    PureWithoutOutputs,
    #[error("function {func_id:?} has no implementation")]
    MissingLambda { func_id: FuncId },
    #[error("function {func_id:?} input {input_idx} has a nil nominal type id")]
    NilInputType { func_id: FuncId, input_idx: usize },
    #[error("function {func_id:?} output {output_idx} has a nil nominal type id")]
    NilOutputType { func_id: FuncId, output_idx: usize },
    #[error(
        "function {func_id:?} output {output_idx} mirrors input {input_idx}, but has {input_count} inputs"
    )]
    InvalidWildcardInput {
        func_id: FuncId,
        output_idx: usize,
        input_idx: usize,
        input_count: usize,
    },
    #[error(
        "function {func_id:?} input {input_idx} declares a default that matches neither its type nor its picker variants"
    )]
    InvalidDefault { func_id: FuncId, input_idx: usize },
}

#[derive(Debug, Error)]
pub enum InvokeError {
    #[error("{0}")]
    External(#[source] Box<dyn error::Error + Send + Sync>),
    #[error("input {index} must be {expected}, got {actual}")]
    InvalidInput {
        index: usize,
        expected: &'static str,
        actual: String,
    },
    /// The lambda bailed because the run was cancelled. The executor maps this
    /// to `execution::Error::Cancelled` (a cancel is not a failure): the node's
    /// output is dropped so it re-runs, and it's reported as cancelled, not
    /// errored. A lambda doing heavy cancellable work returns this when it
    /// observes the cancel token set.
    #[error("cancelled")]
    Cancelled,
}

impl InvokeError {
    pub fn external(error: impl error::Error + Send + Sync + 'static) -> Self {
        Self::External(Box::new(error))
    }

    pub fn invalid_input(index: usize, expected: &'static str, actual: impl fmt::Debug) -> Self {
        Self::InvalidInput {
            index,
            expected,
            actual: format!("{actual:?}"),
        }
    }
}

pub type InvokeResult<T> = Result<T, InvokeError>;
