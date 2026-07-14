use std::pin::Pin;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    DynamicValue,
    runtime::{any_state::AnyState, context::ContextManager, shared_any_state::SharedAnyState},
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

impl OutputUsage {
    pub fn is_skip(self) -> bool {
        matches!(self, OutputUsage::Skip)
    }

    /// Whether this is the *last* remaining read — the one read that, once
    /// counted, leaves nothing else owed. `Skip` (already spent) is never last.
    pub(crate) fn is_last_read(self) -> bool {
        matches!(self, OutputUsage::Needed(1))
    }

    /// Count one more reader against this usage, in place: `Skip` becomes the
    /// first reader (`Needed(1)`), `Needed(n)` steps up to `Needed(n + 1)`. The
    /// planner's backward walk uses this to fold in each consumer edge.
    pub(crate) fn inc(&mut self) {
        *self = match *self {
            OutputUsage::Skip => OutputUsage::Needed(1),
            OutputUsage::Needed(n) => OutputUsage::Needed(n + 1),
        };
    }

    /// Count one read against this usage, in place: `Needed(n)` steps down to
    /// `Needed(n - 1)` or `Skip`. Decrementing a `Skip` output is a planner
    /// bug — nothing should ever read more than the plan counted.
    pub(crate) fn dec(&mut self) {
        *self = match *self {
            OutputUsage::Needed(1) => OutputUsage::Skip,
            OutputUsage::Needed(n) => OutputUsage::Needed(n - 1),
            OutputUsage::Skip => panic!("decremented an OutputUsage the plan already marked Skip"),
        };
    }
}

impl From<usize> for OutputUsage {
    /// `Skip` for a zero count, `Needed(count)` otherwise — the seed for a raw
    /// reader count (a `bool` pinned flag as `0`/`1`, or a plan-level count).
    fn from(count: usize) -> Self {
        if count == 0 {
            OutputUsage::Skip
        } else {
            OutputUsage::Needed(count as u32)
        }
    }
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

/// One resolved input handed to a lambda. The slice is `&mut` so a lambda can
/// `std::mem::take` a value it wants to own (the executor never reads `inputs`
/// again after the invoke); a taken `Custom` value is uniquely held whenever the
/// producer was non-RAM single-consumer (see the executor's move-on-last-use).
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
        &'a mut [InvokeInput],
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
            &'a mut [InvokeInput],
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
        inputs: &mut [InvokeInput],
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
