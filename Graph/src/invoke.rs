use std::collections::HashMap;

pub type Args = Vec<i32>;

pub trait Invoker {
    fn call(&mut self, function_name: &str, context_id: u32, inputs: &Args, outputs: &mut Args);
    fn finish(&mut self);
}

pub trait Invokable {
    fn call(&self, context_id: u32, inputs: &Args, outputs: &mut Args);
}

pub struct Context {
    self_id: u32,
}

pub struct ContextManager {
    contexts: HashMap<u32, Context>,
}

pub struct LambdaInvokable {
    lambda: Box<dyn Fn(u32, &Args, &mut Args)>,
}

pub struct LambdaInvoker {
    lambdas: HashMap<String, LambdaInvokable>,
}

impl LambdaInvoker {
    pub fn new() -> LambdaInvoker {
        LambdaInvoker {
            lambdas: HashMap::new(),
        }
    }

    pub fn add_lambda<F: Fn(u32, &Args, &mut Args) + 'static>(&mut self, function_name: &str, lambda: F) {
        let invokable = LambdaInvokable {
            lambda: Box::new(lambda),
        };
        self.lambdas.insert(function_name.to_string(), invokable);
    }
}

impl Invoker for LambdaInvoker {
    fn call(&mut self, function_name: &str, context_id: u32, inputs: &Args, outputs: &mut Args) {
        if let Some(func) = self.lambdas.get(function_name) {
            (func.lambda)(context_id, inputs, outputs);
        } else {
            panic!("Function not found: {}", function_name);
        }
    }
    fn finish(&mut self) {}
}
