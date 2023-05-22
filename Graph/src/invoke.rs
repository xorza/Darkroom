use std::collections::HashMap;

pub type Args = Vec<i32>;

pub trait Invoker {
    fn call(&self, function_name: &str, inputs: &Args, outputs: &mut Args);
    fn finish(&self);
}

pub trait Invokable {
    fn call(&self, inputs: &Args, outputs: &mut Args);
}

pub struct LambdaInvokable {
    lambda: Box<dyn Fn(&Args, &mut Args)>,
}

pub struct LambdaInvoker {
    lambdas: HashMap<String, LambdaInvokable>,
}

impl LambdaInvoker {
    pub fn new() -> LambdaInvoker {
        LambdaInvoker {
            lambdas: HashMap::new()
        }
    }

    pub fn add_lambda<F: Fn(&Args, &mut Args) + 'static>(&mut self, function_name: &str, lambda: F) {
        let invokable = LambdaInvokable {
            lambda: Box::new(lambda),
        };
        self.lambdas.insert(function_name.to_string(), invokable);
    }
}

impl Invoker for LambdaInvoker {
    fn call(&self, function_name: &str, inputs: &Args, outputs: &mut Args) {
        if let Some(func) = self.lambdas.get(function_name) {
            (func.lambda)(inputs, outputs);
        } else {
            panic!("Function not found: {}", function_name);
        }
    }
    fn finish(&self) {}
}
