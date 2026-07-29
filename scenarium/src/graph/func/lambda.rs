use std::pin::Pin;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::graph::func::error::InvokeResult;
use crate::{
    DynamicValue,
    runtime::{any_state::AnyState, context::ContextManager, shared_any_state::SharedAnyState},
};

/// Whether a node output must be produced for this run. The planner marks an output
/// demanded when a downstream binding reads it or the host requested it through a pin.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputDemand {
    #[default]
    Skip,
    Produce,
}

impl OutputDemand {
    pub fn is_skip(self) -> bool {
        matches!(self, OutputDemand::Skip)
    }
}

/// Everything a lambda is handed for one call: the shared context, its own persistent and
/// event state, the resolved inputs, what the run demands of each output, and the buffer to
/// write them into. One bundle rather than six ordered borrows, so adding invocation state
/// doesn't rewrite every registered lambda.
///
/// `inputs` is `&mut` so a lambda can `std::mem::take` a value it wants to own — the
/// executor never reads them again after the invoke, and a taken `Custom` value is uniquely
/// held whenever the producer was non-RAM single-consumer (see the executor's
/// move-on-last-use).
#[derive(Debug)]
pub struct Invocation<'a> {
    pub ctx: &'a mut ContextManager,
    pub state: &'a mut AnyState,
    pub event_state: &'a SharedAnyState,
    pub inputs: &'a mut [DynamicValue],
    pub demand: &'a [OutputDemand],
    pub outputs: &'a mut [DynamicValue],
}

type AsyncLambdaFuture<'a> = Pin<Box<dyn Future<Output = InvokeResult<()>> + Send + 'a>>;

pub trait AsyncLambdaFn:
    for<'a> Fn(Invocation<'a>) -> AsyncLambdaFuture<'a> + Send + Sync + 'static
{
}

impl<T> AsyncLambdaFn for T where
    T: for<'a> Fn(Invocation<'a>) -> AsyncLambdaFuture<'a> + Send + Sync + 'static
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

    pub async fn invoke(&self, invocation: Invocation<'_>) -> InvokeResult<()> {
        match self {
            FuncLambda::None => {
                panic!("Func missing lambda");
            }
            FuncLambda::Lambda(inner) => (inner)(invocation).await,
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

#[cfg(test)]
pub(crate) mod internals {
    use std::error;
    use std::fmt;

    use crate::graph::func::error::InvokeError;

    #[derive(Debug)]
    struct TestInvokeError(String);

    impl fmt::Display for TestInvokeError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(&self.0)
        }
    }

    impl error::Error for TestInvokeError {}

    pub(crate) fn failure(message: impl Into<String>) -> InvokeError {
        InvokeError::external(TestInvokeError(message.into()))
    }
}
