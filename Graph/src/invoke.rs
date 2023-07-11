use std::collections::HashMap;

use crate::data::Value;
use crate::functions::FunctionId;
use crate::runtime_graph::InvokeContext;

pub type InvokeArgs = [Option<Value>];

pub trait Invokable {
    fn call(&self, ctx: &mut InvokeContext, inputs: &InvokeArgs, outputs: &mut InvokeArgs);
}

pub trait Invoker {
    fn all_functions(&self) -> Vec<FunctionId>;

    fn invoke(
        &self,
        function_id: FunctionId,
        ctx: &mut InvokeContext,
        inputs: &InvokeArgs,
        outputs: &mut InvokeArgs,
    ) -> anyhow::Result<()>;
}


pub type Lambda = dyn Fn(&mut InvokeContext, &InvokeArgs, &mut InvokeArgs) + 'static;

pub struct LambdaInvokable {
    lambda: Box<Lambda>,
}

#[derive(Default)]
pub struct LambdaInvoker {
    all_functions: Vec<FunctionId>,
    lambdas: HashMap<FunctionId, LambdaInvokable>,
}

impl LambdaInvoker {
    pub fn add_lambda<F>(&mut self, function_id: FunctionId, lambda: F)
    where F: Fn(&mut InvokeContext, &InvokeArgs, &mut InvokeArgs) + 'static
    {
        let invokable = LambdaInvokable {
            lambda: Box::new(lambda),
        };
        self.lambdas.insert(function_id, invokable);
        self.all_functions.push(function_id);
    }
}

impl Invoker for LambdaInvoker {
    fn all_functions(&self) -> Vec<FunctionId> {
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
