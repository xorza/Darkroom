use std::collections::HashMap;
use std::ops::{Index, IndexMut};

use uuid::Uuid;

use crate::data::{DataType, Value};

#[derive(Clone, Default)]
pub struct InvokeArgs {
    values: Vec<Value>,
}

pub trait Invoker {
    fn start(&self) {}
    fn call(&self, function_id: Uuid, context_id: Uuid, inputs: &InvokeArgs, outputs: &mut InvokeArgs) -> anyhow::Result<()>;
    fn finish(&self) {}
}

pub trait Invokable {
    fn call(&self, context_id: Uuid, inputs: &InvokeArgs, outputs: &mut InvokeArgs);
}

pub struct LambdaInvokable {
    lambda: Box<dyn Fn(Uuid, &InvokeArgs, &mut InvokeArgs)>,
}

pub struct LambdaInvoker {
    lambdas: HashMap<Uuid, LambdaInvokable>,
}

impl LambdaInvoker {
    pub fn new() -> LambdaInvoker {
        LambdaInvoker {
            lambdas: HashMap::new(),
        }
    }

    pub fn add_lambda<F: Fn(Uuid, &InvokeArgs, &mut InvokeArgs) + 'static>(&mut self, function_id: Uuid, lambda: F) {
        let invokable = LambdaInvokable {
            lambda: Box::new(lambda),
        };
        self.lambdas.insert(function_id, invokable);
    }
}

impl Invoker for LambdaInvoker {
    fn start(&self) {}
    fn call(&self, function_id: Uuid, context_id: Uuid, inputs: &InvokeArgs, outputs: &mut InvokeArgs) -> anyhow::Result<()> {
        let func = self.lambdas
            .get(&function_id)
            .ok_or(anyhow::anyhow!("Function not found: {}", function_id))?;

        (func.lambda)(context_id, inputs, outputs);

        Ok(())
    }
    fn finish(&self) {}
}


impl InvokeArgs {
    pub fn new() -> InvokeArgs {
        InvokeArgs {
            values: Vec::new(),
        }
    }
    pub fn with_size(size: usize) -> InvokeArgs {
        let mut result = InvokeArgs {
            values: Vec::with_capacity(size),
        };
        result.values.resize(size, Value::Null);
        result
    }

    fn resize(&mut self, size: usize) {
        self.values.resize(size, Value::Null);
    }

    pub fn iter(&self) -> std::slice::Iter<Value> {
        self.values.iter()
    }

    pub fn from_vec<T: Into<Value>>(values: Vec<T>) -> InvokeArgs {
        let mut result = InvokeArgs::new();
        for value in values {
            result.values.push(value.into());
        }

        result
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }
}

impl Index<usize> for InvokeArgs {
    type Output = Value;

    fn index(&self, idx: usize) -> &Value {
        &self.values[idx]
    }
}

impl IndexMut<usize> for InvokeArgs {
    fn index_mut(&mut self, index: usize) -> &mut Value {
        &mut self.values[index]
    }
}

