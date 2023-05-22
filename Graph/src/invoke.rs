use std::collections::HashMap;

#[derive(Clone)]
pub struct Args {
    pub inputs: Vec<i32>,
    pub outputs: Vec<i32>,
}

pub trait Invoker {
    fn call(&self, function_name: &str, args: &mut Args);
    fn finish(&self);
}

pub trait Invokable {
    fn call(&self, args: &mut Args);
}

pub struct LambdaInvokable {
    lambda: Box<dyn Fn(&mut Args)>,
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

    pub fn add_lambda<F: Fn(&mut Args) + 'static>(&mut self, function_name: &str, lambda: F) {
        let invokable = LambdaInvokable {
            lambda: Box::new(lambda),
        };
        self.lambdas.insert(function_name.to_string(), invokable);
    }
}

impl Invoker for LambdaInvoker {
    fn call(&self, function_name: &str, args: &mut Args) {
        if let Some(func) = self.lambdas.get(function_name) {
            (func.lambda)(args);
        } else {
            panic!("Function not found: {}", function_name);
        }
    }
    fn finish(&self) {}
}

impl Args {
    pub fn new() -> Args {
        Args {
            inputs: Vec::new(),
            outputs: Vec::new(),
        }
    }
}