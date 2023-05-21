use std::collections::HashMap;

pub trait Invoker {
    fn call(&self, function_name: &str, inputs: &Vec<i32>, outputs: &mut Vec<i32>);
}

pub struct LambdaInvoker {
    lambdas: HashMap<String, Box<dyn Fn(&Vec<i32>, &mut Vec<i32>)>>,
}

impl LambdaInvoker {
    pub fn new() -> LambdaInvoker {
        LambdaInvoker {
            lambdas: HashMap::new()
        }
    }

    pub fn add_lambda<F: Fn(&Vec<i32>, &mut Vec<i32>) + 'static>(&mut self, function_name: &str, lambda: F) {
        self.lambdas.insert(function_name.to_string(), Box::new(lambda));
    }
}

impl Invoker for LambdaInvoker {
    fn call(&self, function_name: &str, inputs: &Vec<i32>, outputs: &mut Vec<i32>) {
        if let Some(func) = self.lambdas.get(function_name) {
            func(inputs, outputs);
        } else {
            panic!("Function not found: {}", function_name);
        }
    }
}