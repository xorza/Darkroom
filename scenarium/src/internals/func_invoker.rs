//! [`FuncInvoker`]: one func's lambda called on its own, outside any graph.

use crate::data::dynamic_value::DynamicValue;
use crate::decl::func::Func;
use crate::decl::func::error::InvokeResult;
use crate::decl::func::func_lambda::{Invocation, OutputDemand};
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

#[cfg(test)]
mod tests {
    use crate::async_lambda;
    use crate::data::const_value::ConstValue;
    use crate::data::data_type::DataType;
    use crate::data::dynamic_value::DynamicValue;
    use crate::decl::func::Func;
    use crate::decl::func::error::InvokeError;
    use crate::decl::func::func_lambda::Invocation;
    use crate::decl::ports::{InputDecl, OutputDecl};
    use crate::identity::FuncId;
    use crate::internals::func_invoker::FuncInvoker;
    use crate::library::Library;

    /// A func computes through the invoker, and its node state carries between
    /// calls the way it does between two runs of one node.
    #[tokio::test]
    async fn an_invoked_func_keeps_its_node_state_between_calls() -> Result<(), InvokeError> {
        let mut library = Library::default();
        library.add(
            Func::builder(
                FuncId::unique(),
                "sum",
                async_lambda!(|Invocation {
                                   state,
                                   inputs,
                                   outputs,
                                   ..
                               }| {
                    let total = inputs[0].as_i64().unwrap() + inputs[1].as_i64().unwrap();
                    state.set(total);
                    outputs[0] = ConstValue::Int(total).into();
                    Ok(())
                }),
            )
            .pure()
            .input(1, InputDecl::required("A", DataType::Int))
            .input(2, InputDecl::required("B", DataType::Int))
            .output(1, OutputDecl::new("Sum", DataType::Int))
            .build(),
        );
        let sum = library.by_name("sum").unwrap();
        let int = |value: i64| DynamicValue::Static(ConstValue::Int(value));
        let mut node = FuncInvoker::default();

        let outputs = node.call(sum, [int(2), int(4)]).await?;
        assert_eq!(outputs[0].as_i64().unwrap(), 6);
        assert_eq!(
            node.state::<i64>(),
            Some(&6),
            "the body stashed its result in the node's own state"
        );

        let outputs = node.call(sum, [int(3), int(5)]).await?;
        assert_eq!(outputs[0].as_i64().unwrap(), 8);
        assert_eq!(
            node.state::<i64>(),
            Some(&8),
            "and the second call replaced it"
        );

        Ok(())
    }
}
