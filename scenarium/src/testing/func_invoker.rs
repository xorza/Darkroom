//! [`FuncInvoker`]: one func's lambda called on its own, outside any graph.

use crate::DynamicValue;
use crate::graph::func::Func;
use crate::graph::func::error::InvokeResult;
use crate::graph::func::lambda::{Invocation, OutputDemand};
use crate::runtime::any_state::AnyState;
use crate::runtime::context::ContextManager;
use crate::runtime::shared_any_state::SharedAnyState;

/// The five collaborators a lambda is handed, held so a test can just say what
/// it passes in.
///
/// A body's own arithmetic — what `Add` computes, what `Concat` joins — needs no
/// graph, no schedule and no cache, only an invoke. But an invoke takes a
/// context, the node's persistent state, its shared event state, the run's
/// per-output demand, and a buffer to write into, so every library test that
/// wanted one stood all five up by hand.
///
/// **State persists across calls**, because a node's does: two calls on one
/// invoker see the same [`AnyState`] and the same [`SharedAnyState`], the way
/// two runs of one node would. A test that wants a node with no history builds
/// a second invoker.
#[derive(Debug, Default)]
pub(crate) struct FuncInvoker {
    ctx: ContextManager,
    state: AnyState,
    event_state: SharedAnyState,
}

impl FuncInvoker {
    /// Invoke `func` on `inputs`, demanding every output it declares, and hand
    /// back what it wrote — `Unbound` for a port it left alone.
    pub(crate) async fn call(
        &mut self,
        func: &Func,
        inputs: impl IntoIterator<Item = DynamicValue>,
    ) -> InvokeResult<Vec<DynamicValue>> {
        self.call_demanding(func, inputs, OutputDemand::Produce)
            .await
    }

    /// [`call`](Self::call) with every output on `demand` — for a body that
    /// reads it and produces less.
    pub(crate) async fn call_demanding(
        &mut self,
        func: &Func,
        inputs: impl IntoIterator<Item = DynamicValue>,
        demand: OutputDemand,
    ) -> InvokeResult<Vec<DynamicValue>> {
        let mut inputs: Vec<DynamicValue> = inputs.into_iter().collect();
        let demand = vec![demand; func.outputs.len()];
        let mut outputs = vec![DynamicValue::Unbound; func.outputs.len()];
        func.lambda
            .invoke(Invocation {
                ctx: &mut self.ctx,
                state: &mut self.state,
                event_state: &self.event_state,
                inputs: &mut inputs,
                demand: &demand,
                outputs: &mut outputs,
            })
            .await?;
        Ok(outputs)
    }

    /// What the calls so far left in the node's own state.
    pub(crate) fn state<T: Send + Sync + 'static>(&self) -> Option<&T> {
        self.state.get::<T>()
    }

    /// The shared state this node's *event* lambdas read — handed out cloned,
    /// since driving an event while the body keeps being called is the whole
    /// point of the two being separate.
    pub(crate) fn event_state(&self) -> SharedAnyState {
        self.event_state.clone()
    }
}
