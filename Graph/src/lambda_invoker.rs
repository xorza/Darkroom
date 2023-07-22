use std::collections::HashMap;

use crate::function::{Function, FunctionId};
use crate::invoke::{InvokeArgs, Invoker};
use crate::runtime_graph::InvokeContext;

pub type Lambda = dyn Fn(&mut InvokeContext, &InvokeArgs, &mut InvokeArgs) + 'static;

pub struct LambdaInvokable {
    lambda: Box<Lambda>,
}

#[derive(Default)]
pub struct LambdaInvoker {
    all_functions: Vec<Function>,
    lambdas: HashMap<FunctionId, LambdaInvokable>,
}

impl LambdaInvoker {
    pub fn add_lambda<F>(&mut self, function: Function, lambda: F)
    where F: Fn(&mut InvokeContext, &InvokeArgs, &mut InvokeArgs) + 'static
    {
        if self.lambdas.contains_key(&function.self_id) {
            panic!("Function {}:{} with the same id already exists.", function.self_id, function.name);
        }

        let invokable = LambdaInvokable {
            lambda: Box::new(lambda),
        };
        self.lambdas.insert(function.self_id, invokable);

        self.all_functions.push(function);
    }
}

impl Invoker for LambdaInvoker {
    fn all_functions(&self) -> Vec<Function> {
        self.all_functions.clone()
    }

    fn invoke(&self,
              function_id: FunctionId,
              ctx: &mut InvokeContext,
              inputs: &InvokeArgs,
              outputs: &mut InvokeArgs)
        -> anyhow::Result<()>
    {
        let invokable = self.lambdas.get(&function_id).unwrap();
        (invokable.lambda)(ctx, inputs, outputs);

        Ok(())
    }
}
