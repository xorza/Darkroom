//! [`FuncInvoker`]: one func's lambda called on its own, outside any graph.

use crate::data::dynamic_value::DynamicValue;
use crate::graph::func::Func;
use crate::graph::func::error::InvokeResult;
use crate::graph::func::lambda::{Invocation, OutputDemand};
use crate::runtime::any_state::AnyState;
use crate::runtime::ctx_manager::CtxManager;
use crate::runtime::shared_any_state::SharedAnyState;

/// The collaborators a lambda is handed (context, the node's state, its shared event state, the
/// per-output demand, a buffer to write into), held so a body's own arithmetic can be tested
/// without a graph, schedule or cache.
///
/// **State persists across calls**, as a node's does across runs: two calls on one invoker see
/// the same [`AnyState`] and [`SharedAnyState`]. A node with no history needs a new invoker.
#[derive(Debug, Default)]
pub struct FuncInvoker {
    ctx: CtxManager,
    state: AnyState,
    event_state: SharedAnyState,
}

impl FuncInvoker {
    /// Invoke `func` on `inputs`, demanding every output it declares, and hand
    /// back what it wrote — `Unbound` for a port it left alone.
    pub async fn call(
        &mut self,
        func: &Func,
        inputs: impl IntoIterator<Item = DynamicValue>,
    ) -> InvokeResult<Vec<DynamicValue>> {
        let mut inputs: Vec<DynamicValue> = inputs.into_iter().collect();
        let demand = vec![OutputDemand::Produce; func.outputs().len()];
        let mut outputs = vec![DynamicValue::Unbound; func.outputs().len()];
        func.lambda()
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
    pub fn state<T: Send + Sync + 'static>(&self) -> Option<&T> {
        self.state.get::<T>()
    }

    /// The shared state this node's *event* lambdas read, handed out cloned so an event can be
    /// driven while the body keeps being called.
    pub fn event_state(&self) -> SharedAnyState {
        self.event_state.clone()
    }

    /// The context the calls run under, for a test that installs what a body
    /// reads from it.
    pub const fn ctx_mut(&mut self) -> &mut CtxManager {
        &mut self.ctx
    }
}
