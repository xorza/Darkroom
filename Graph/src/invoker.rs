use std::collections::HashMap;
use std::ops::{Index, IndexMut};

use uuid::Uuid;

use crate::data::{DataType, Value};
use crate::graph::{Binding, Graph};
use crate::runtime::RuntimeInfo;

#[derive(Clone, Default, Hash, PartialEq, Eq)]
struct OutputAddress {
    node_id: Uuid,
    output_index: u32,
}

pub(crate) type InvokeArgs = [Option<Value>];

#[derive(Default)]
pub(crate) struct ArgSet {
    args: Vec<Option<Value>>,
}


pub trait Invoker {
    fn run(&self, graph: &Graph, runtime: &RuntimeInfo) -> anyhow::Result<()> {
        let mut arg_cache: HashMap<OutputAddress, Value> = HashMap::new();
        let mut inputs: ArgSet = ArgSet::default();
        let mut outputs: ArgSet = ArgSet::default();

        for r_node in runtime.nodes.iter()
            .filter(|node| node.should_execute) {
            let _execution_index = r_node.execution_index.unwrap();
            assert!(r_node.execution_index.is_some());

            let node = graph.node_by_id(r_node.node_id()).unwrap();
            inputs.resize_and_fill(node.inputs.len());
            outputs.resize_and_fill(node.outputs.len());

            node.inputs.iter()
                .map(|input| {
                    match &input.binding {
                        Binding::None => None,
                        Binding::Const => input.const_value.clone(),
                        Binding::Output(output_binding) => {
                            let output_address = OutputAddress {
                                node_id: output_binding.output_node_id,
                                output_index: output_binding.output_index,
                            };

                            arg_cache.get(&output_address).cloned()
                        }
                    }
                })
                .enumerate()
                .for_each(|(index, value)| inputs[index] = value);

            self.invoke(node.function_id, node.id(), inputs.as_slice(), outputs.as_mut_slice())?;

            outputs.iter()
                .enumerate()
                .filter(|(_, value)| value.is_some())
                .for_each(|(index, value)| {
                    let insert_result = arg_cache.insert(
                        OutputAddress {
                            node_id: node.id(),
                            output_index: index as u32,
                        },
                        value.clone().unwrap());
                    assert!(insert_result.is_none());
                });
        }

        Ok(())
    }

    fn invoke(&self,
              function_id: Uuid,
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
    fn invoke(&self,
              function_id: Uuid,
              context_id: Uuid,
              inputs: &InvokeArgs,
              outputs: &mut InvokeArgs)
        -> anyhow::Result<()>
    {
        let invokable = self.lambdas.get(&function_id).unwrap();
        (invokable.lambda)(context_id, inputs, outputs);

        Ok(())
    }
}


impl ArgSet {
    pub fn from_vec<T, V>(args: Vec<T>) -> Self
        where T: Into<Option<V>>,
              V: Into<Value>
    {
        Self {
            args: args.into_iter().map(|v| v.into().map(|v| v.into())).collect(),
        }
    }
    pub fn as_slice(&self) -> &[Option<Value>] {
        self.args.as_slice()
    }
    pub fn as_mut_slice(&mut self) -> &mut [Option<Value>] {
        self.args.as_mut_slice()
    }
    pub fn resize_and_fill(&mut self, size: usize) {
        self.args.resize(size, None);
        self.args.fill(None);
    }
    pub fn iter(&self) -> impl Iterator<Item=&Option<Value>> {
        self.args.iter()
    }
    pub fn iter_mut(&mut self) -> impl Iterator<Item=&mut Option<Value>> {
        self.args.iter_mut()
    }
}

impl Index<usize> for ArgSet {
    type Output = Option<Value>;

    fn index(&self, index: usize) -> &Self::Output {
        &self.args[index]
    }
}

impl IndexMut<usize> for ArgSet {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.args[index]
    }
}
