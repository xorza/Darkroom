use std::pin::Pin;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    context::ContextManager,
    data::DynamicValue,
    execution::digest::Digest,
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

type PreCheckFn = dyn Fn(&mut AnyState, &[InvokeInput]) -> Digest + Send + Sync + 'static;

/// Optional probe, run before a node's main lambda, that returns a **content digest**
/// of whatever the func decides identifies its output — typically a fingerprint of the
/// files/inputs it actually reads (`Digest::hash` builds one). The framework compares
/// it to the digest the node's resident output was produced under: an unchanged digest
/// with clean upstream lets the executor reuse the prior output and skip the lambda,
/// and — because it's a content key, not just a "did it change" bit — it's what a disk
/// cache can be keyed on. Runs at execution time, so it sees resolved inputs (incl.
/// bound paths); the framework owns storage/comparison, so the func does no
/// bookkeeping. Stored on the func like [`FuncLambda`], skipped on serialize and
/// re-attached at flatten.
#[derive(Clone, Default)]
pub enum PreCheck {
    #[default]
    None,
    Check(Arc<PreCheckFn>),
}

impl PreCheck {
    pub fn new<F>(check: F) -> Self
    where
        F: Fn(&mut AnyState, &[InvokeInput]) -> Digest + Send + Sync + 'static,
    {
        Self::Check(Arc::new(check))
    }

    pub fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }

    /// Run the probe, or `None` for a node without a pre-check (which then runs every
    /// time it's scheduled — the default behavior). `state` is the node's persistent
    /// [`AnyState`] — the framework owns the digest comparison, so the func needn't use
    /// it, but it's threaded through for funcs that want to cache intermediate work.
    pub fn check(&self, state: &mut AnyState, inputs: &[InvokeInput]) -> Option<Digest> {
        match self {
            PreCheck::None => None,
            PreCheck::Check(check) => Some(check(state, inputs)),
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
