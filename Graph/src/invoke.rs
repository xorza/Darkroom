use std::collections::HashMap;

use crate::data::DynamicValue;
use crate::function::{Function, FunctionId};
use crate::runtime_graph::InvokeContext;

pub type InvokeArgs = [Option<DynamicValue>];

pub trait Invokable {
    fn call(&self, ctx: &mut InvokeContext, inputs: &InvokeArgs, outputs: &mut InvokeArgs);
}

pub trait Invoker {
    fn all_functions(&self) -> Vec<Function>;

    fn invoke(
        &self,
        function_id: FunctionId,
        ctx: &mut InvokeContext,
        inputs: &InvokeArgs,
        outputs: &mut InvokeArgs,
    ) -> anyhow::Result<()>;
}

pub struct UberInvoker {
    invokers: Vec<Box<dyn Invoker>>,
    function_id_to_invoker_index: HashMap<FunctionId, usize>,
}

impl UberInvoker {
    pub fn new(invokers: Vec<Box<dyn Invoker>>) -> Self {
        let mut function_id_to_invoker_index = HashMap::new();
        invokers
            .iter()
            .enumerate()
            .for_each(|(index, invoker)| {
                invoker
                    .all_functions()
                    .iter()
                    .for_each(|function| {
                        function_id_to_invoker_index.insert(function.self_id, index);
                    });
            });

        Self {
            invokers,
            function_id_to_invoker_index,
        }
    }
}

impl Invoker for UberInvoker {
    fn all_functions(&self) -> Vec<Function> {
        self.invokers.iter().flat_map(|invoker| invoker.all_functions()).collect()
    }

    fn invoke(
        &self,
        function_id: FunctionId,
        ctx: &mut InvokeContext,
        inputs: &InvokeArgs,
        outputs: &mut InvokeArgs,
    ) -> anyhow::Result<()>
    {
        let invoker_index = self.function_id_to_invoker_index
            .get(&function_id)
            .ok_or_else(|| anyhow::anyhow!("No invoker found for function_id: {:?}", function_id))?;

        let invoker = &self.invokers[*invoker_index];
        invoker.invoke(function_id, ctx, inputs, outputs)
    }
}
