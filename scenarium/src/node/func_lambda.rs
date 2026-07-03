use std::pin::Pin;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    data::DynamicValue,
    prelude::{AnyState, SharedAnyState},
    runtime::context::ContextManager,
};

/// How much of a node output a run actually needs, handed to the lambda so it can
/// skip producing values nobody reads. Derived per-run by the executor from the
/// plan's consumer counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputUsage {
    Skip,
    /// Number of executing consumers reading this output this run (always `> 0`).
    Needed(u32),
}

#[derive(Debug, Error)]
pub enum InvokeError {
    #[error("{0}")]
    External(#[from] anyhow::Error),
    /// The lambda bailed because the run was cancelled. The executor maps this
    /// to `execution::Error::Cancelled` (a cancel is not a failure): the node's
    /// output is dropped so it re-runs, and it's reported as cancelled, not
    /// errored. A lambda doing heavy cancellable work returns this when it
    /// observes the cancel token set.
    #[error("cancelled")]
    Cancelled,
}

pub type InvokeResult<T> = Result<T, InvokeError>;

#[derive(Debug)]
pub struct InvokeInput {
    pub value: DynamicValue,
}

type AsyncLambdaFuture<'a> = Pin<Box<dyn Future<Output = InvokeResult<()>> + Send + 'a>>;

pub trait AsyncLambdaFn:
    for<'a> Fn(
        &'a mut ContextManager,
        &'a mut AnyState,
        &'a SharedAnyState,
        &'a [InvokeInput],
        &'a [OutputUsage],
        &'a mut [DynamicValue],
    ) -> AsyncLambdaFuture<'a>
    + Send
    + Sync
    + 'static
{
}

impl<T> AsyncLambdaFn for T where
    T: for<'a> Fn(
            &'a mut ContextManager,
            &'a mut AnyState,
            &'a SharedAnyState,
            &'a [InvokeInput],
            &'a [OutputUsage],
            &'a mut [DynamicValue],
        ) -> AsyncLambdaFuture<'a>
        + Send
        + Sync
        + 'static
{
}

pub type AsyncLambda = dyn AsyncLambdaFn;

#[derive(Clone, Default)]
pub enum FuncLambda {
    #[default]
    None,
    Lambda(Arc<AsyncLambda>),
}

impl FuncLambda {
    pub fn new<F>(lambda: F) -> Self
    where
        F: AsyncLambdaFn,
    {
        Self::Lambda(Arc::new(lambda))
    }

    pub fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }

    pub async fn invoke(
        &self,
        ctx_manager: &mut ContextManager,
        state: &mut AnyState,
        event_state: &SharedAnyState,
        inputs: &[InvokeInput],
        output_usage: &[OutputUsage],
        outputs: &mut [DynamicValue],
    ) -> InvokeResult<()> {
        match self {
            FuncLambda::None => {
                panic!("Func missing lambda");
            }
            FuncLambda::Lambda(inner) => {
                (inner)(
                    ctx_manager,
                    state,
                    event_state,
                    inputs,
                    output_usage,
                    outputs,
                )
                .await
            }
        }
    }
}

impl std::fmt::Debug for FuncLambda {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FuncLambda::None => f.debug_struct("FuncLambda::None").finish(),
            FuncLambda::Lambda(_) => f.debug_struct("FuncLambda::Lambda").finish(),
        }
    }
}
