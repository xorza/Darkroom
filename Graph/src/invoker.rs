use std::collections::HashMap;
use std::ops::{Index, IndexMut};

use uuid::Uuid;

use crate::data::{DataType, Value};
use crate::graph::Graph;
use crate::runtime::RuntimeInfo;

#[derive(Clone, Default)]
pub struct InvokeArgs {
    values: Vec<Value>,
}

#[derive(Clone, Default, Hash, PartialEq, Eq)]
struct InputAddress {
    node_id: Uuid,
    output_index: u32,
}


pub trait Invoker {
    fn run(&self, graph: &Graph, runtime: &RuntimeInfo) -> anyhow::Result<()> {
        for r_node in runtime.nodes.iter()
            .filter(|node| node.should_execute) {
            assert!(r_node.execution_index.is_some());
            
            let node = graph.node_by_id(r_node.node_id()).unwrap();
            let input_args = InvokeArgs::with_size(node.inputs.len() as u32);
            
            for (index, input) in node.inputs.iter().enumerate() {
                
            }

            r_node.node_id();
        }

        Ok(())
    }

    fn invoke(function_id: Uuid,
              context_id: Uuid,
              inputs: &InvokeArgs,
              outputs: &mut InvokeArgs)
              -> anyhow::Result<()>;
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
    fn invoke(function_id: Uuid, context_id: Uuid, inputs: &InvokeArgs, outputs: &mut InvokeArgs) -> anyhow::Result<()> {
        todo!()
    }
}


impl InvokeArgs {
    pub fn new() -> InvokeArgs {
        InvokeArgs {
            values: Vec::new(),
        }
    }
    pub fn with_size(size: u32) -> InvokeArgs {
        InvokeArgs {
            values: vec![Value::Null; size as usize],
        }
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

