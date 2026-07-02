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

type DigestFn = dyn Fn(&mut AnyState, &[InvokeInput]) -> Option<Digest> + Send + Sync + 'static;

/// What a node's pre-check yielded, telling `node_digest` how to key the node. Produced by
/// [`PreCheck::run`].
#[derive(Debug, Clone, Copy)]
pub enum PreCheckDigest {
    /// No pre-check on this node — fold its inputs structurally.
    None,
    /// The pre-check computed the node's content digest — the framework keys the node on it
    /// alone, folding only func id/version + output types (no structural input fold).
    Computed(Digest),
    /// The pre-check declined (no valid key this run) — the node isn't cacheable and always
    /// recomputes.
    Uncacheable,
}

/// Optional probe a func supplies to **compute its node's content digest** from the node's
/// resolved inputs, *replacing* the framework's structural digest. Returning `Some(digest)`
/// keys the node on that digest; `None` marks it uncacheable this run.
///
/// This keys the node on the *value* of its inputs, not the computation that produced them,
/// so the node reuses across upstream changes that don't change its effective inputs (e.g.
/// `build_masters` keys on frame *content*; the file-cache node on its path alone). In
/// exchange, the func **owns the whole key**: it must fingerprint *every* output-affecting
/// input (files via [`Digest::fs_path`](crate::execution::digest::Digest::fs_path), scalars
/// via their bytes). Two consequences: an opaque `Custom` value arriving on a `Bind` can't
/// be fingerprinted (so a node depending on one can't use a pre-check), and impurity taint
/// isn't automatic — a value the probe doesn't fold won't invalidate the node.
///
/// The framework compares the result to the digest the node's resident output was produced
/// under; an unchanged digest lets the executor reuse the prior output and skip the lambda,
/// and — being a content key — it's what a disk cache is keyed on. Runs at execution time,
/// so it sees resolved inputs (incl. bound paths). Stored on the func like [`FuncLambda`],
/// skipped on serialize and re-attached at flatten.
#[derive(Clone, Default)]
pub enum PreCheck {
    #[default]
    None,
    Compute(Arc<DigestFn>),
}

impl PreCheck {
    /// A pre-check that computes the node's entire content digest from its resolved inputs
    /// (`Some(digest)` ⇒ key on it; `None` ⇒ not cacheable this run). `state` is the node's
    /// persistent [`AnyState`] — the framework owns the comparison, so the func needn't use
    /// it, but it's threaded through for funcs that cache intermediate work. The func must
    /// fingerprint every output-affecting input (see the type docs).
    pub fn compute<F>(compute: F) -> Self
    where
        F: Fn(&mut AnyState, &[InvokeInput]) -> Option<Digest> + Send + Sync + 'static,
    {
        Self::Compute(Arc::new(compute))
    }

    pub fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }

    /// Run the probe and classify the result (see [`PreCheckDigest`]). [`None`](PreCheck::None)
    /// yields [`PreCheckDigest::None`], so a node without a pre-check runs every time it's
    /// scheduled unless it's `Pure` (the default behavior).
    pub fn run(&self, state: &mut AnyState, inputs: &[InvokeInput]) -> PreCheckDigest {
        match self {
            PreCheck::None => PreCheckDigest::None,
            PreCheck::Compute(compute) => match compute(state, inputs) {
                Some(digest) => PreCheckDigest::Computed(digest),
                None => PreCheckDigest::Uncacheable,
            },
        }
    }
}

impl std::fmt::Debug for PreCheck {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PreCheck::None => f.debug_struct("PreCheck::None").finish(),
            PreCheck::Compute(_) => f.debug_struct("PreCheck::Compute").finish(),
        }
    }
}
