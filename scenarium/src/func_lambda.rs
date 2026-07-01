use std::pin::Pin;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    context::ContextManager,
    data::DynamicValue,
    prelude::{AnyState, SharedAnyState},
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

/// A func's pre-check verdict, returned by [`PreCheck`] before the main lambda runs.
/// `Unchanged` lets the executor reuse the node's prior output and skip the lambda;
/// `Changed` runs it. A func returns `Unchanged` only when it can guarantee its output
/// is identical to last run's — the same determinism contract as `Pure`, asserted at
/// runtime rather than declared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeCheck {
    Changed,
    Unchanged,
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

type PreCheckFn = dyn Fn(&mut AnyState, &[InvokeInput]) -> ChangeCheck + Send + Sync + 'static;

/// Optional cheap "did anything I read change?" probe, run before a node's main
/// lambda. Given the node's persistent [`AnyState`] (to recall + update a fingerprint
/// of what it last read) and the resolved inputs, it returns a [`ChangeCheck`]. When
/// it reports `Unchanged` and the node's upstream producers are all unchanged this
/// run, the executor skips the lambda and reuses the prior output — and the skip
/// propagates to pre-check consumers. Stored on the func like [`FuncLambda`], skipped
/// on serialize and re-attached at flatten.
#[derive(Clone, Default)]
pub enum PreCheck {
    #[default]
    None,
    Check(Arc<PreCheckFn>),
}

impl PreCheck {
    pub fn new<F>(check: F) -> Self
    where
        F: Fn(&mut AnyState, &[InvokeInput]) -> ChangeCheck + Send + Sync + 'static,
    {
        Self::Check(Arc::new(check))
    }

    pub fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }

    /// Run the probe. A node with no pre-check is always treated as `Changed`, so it
    /// runs every time it's scheduled (the default behavior).
    pub fn check(&self, state: &mut AnyState, inputs: &[InvokeInput]) -> ChangeCheck {
        match self {
            PreCheck::None => ChangeCheck::Changed,
            PreCheck::Check(check) => check(state, inputs),
        }
    }
}

impl std::fmt::Debug for PreCheck {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PreCheck::None => f.debug_struct("PreCheck::None").finish(),
            PreCheck::Check(_) => f.debug_struct("PreCheck::Check").finish(),
        }
    }
}
