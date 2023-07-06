use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::{Index, IndexMut};

use uuid::Uuid;

use crate::data::{DataType, Value};
use crate::functions::FunctionId;
use crate::graph::{Binding, Graph, NodeId};
use crate::preprocess::PreprocessInfo;

#[derive(Clone, Default, Hash, PartialEq, Eq)]
pub struct OutputAddress {
    node_id: NodeId,
    output_index: u32,
}

pub(crate) type InvokeArgs = [Option<Value>];

#[derive(Default)]
pub(crate) struct ArgSet {
    args: Vec<Option<Value>>,
}

pub type ArgCache = HashMap<OutputAddress, Option<Value>>;

pub struct NodeInvokeInfo {
    node_id: NodeId,
    runtime: f64,
}

#[derive(Default)]
pub struct ComputeInfo {
    arg_cache: ArgCache,
    node_invoke_infos: Vec<NodeInvokeInfo>,
    context_cache: HashMap<NodeId, RefCell<InvokeContext>>,
}

pub struct InvokeContext {
    boxed: Option<Box<dyn Any>>,
}

pub trait Invokable {
    fn call(&self, ctx: &mut InvokeContext, inputs: &InvokeArgs, outputs: &mut InvokeArgs);
}

pub trait Compute {
    fn run(&self,
           graph: &Graph,
           preprocess_info: &PreprocessInfo,
           prev_compute_info: &ComputeInfo)
           -> anyhow::Result<ComputeInfo>
    {
        let mut compute_info = ComputeInfo::default();
        let mut inputs: ArgSet = ArgSet::default();
        let mut outputs: ArgSet = ArgSet::default();

        for r_node in preprocess_info.nodes.iter()
            .filter(|node| node.should_execute) {
            let _execution_index = r_node.execution_index.unwrap();
            assert!(r_node.execution_index.is_some());

            let node = graph.node_by_id(r_node.node_id()).unwrap();
            inputs.resize_and_fill(node.inputs.len());
            outputs.resize_and_fill(node.outputs.len());

            node.inputs.iter()
                .map(|input| {
                    match &input.binding {
                        Binding::None =>
                            None,

                        Binding::Const =>
                            input.const_value.clone(),

                        Binding::Output(output_binding) => {
                            let output_address = OutputAddress {
                                node_id: output_binding.output_node_id,
                                output_index: output_binding.output_index,
                            };

                            compute_info.arg_cache
                                .get(&output_address)
                                .or_else(|| prev_compute_info.arg_cache.get(&output_address))
                                .unwrap_or_else(|| {
                                    panic!("Output {:?} not found for node {:?}, id: {:?}", output_address.output_index, node.name, node.id())
                                })
                                .clone()
                        }
                    }
                })
                .enumerate()
                .for_each(|(index, value)| inputs[index] = value);

            let mut ctx = prev_compute_info.context_cache
                .get(&node.id())
                .and_then(|ctx|
                    Some(ctx.replace(InvokeContext::default()))
                )
                .unwrap_or_else(|| InvokeContext::default());

            let start = std::time::Instant::now();
            self.invoke(node.function_id, &mut ctx, inputs.as_slice(), outputs.as_mut_slice())?;
            let elapsed = start.elapsed();

            let insert_result
                = compute_info.context_cache.insert(node.id(), RefCell::new(ctx));
            assert!(insert_result.is_none());

            compute_info.node_invoke_infos.push(NodeInvokeInfo {
                node_id: node.id(),
                runtime: elapsed.as_secs_f64(),
            });

            outputs.iter()
                .enumerate()
                .for_each(|(index, value)| {
                    let insert_result = compute_info.arg_cache
                        .insert(
                            OutputAddress {
                                node_id: node.id(),
                                output_index: index as u32,
                            },
                            value.clone(),
                        );
                    assert!(insert_result.is_none());
                });
        }

        Ok(compute_info)
    }

    fn invoke(&self,
              function_id: FunctionId,
              ctx: &mut InvokeContext,
              inputs: &InvokeArgs,
              outputs: &mut InvokeArgs)
              -> anyhow::Result<()>;
}

pub type Lambda = dyn Fn(&mut InvokeContext, &InvokeArgs, &mut InvokeArgs) + 'static;

pub struct LambdaInvokable {
    lambda: Box<Lambda>,
}

pub struct LambdaCompute {
    lambdas: HashMap<FunctionId, LambdaInvokable>,
}

impl LambdaCompute {
    pub fn new() -> LambdaCompute {
        LambdaCompute {
            lambdas: HashMap::new(),
        }
    }

    pub fn add_lambda<F>(&mut self, function_id: FunctionId, lambda: F)
        where F: Fn(&mut InvokeContext, &InvokeArgs, &mut InvokeArgs) + 'static
    {
        let invokable = LambdaInvokable {
            lambda: Box::new(lambda),
        };
        self.lambdas.insert(function_id, invokable);
    }
}

impl Compute for LambdaCompute {
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

impl InvokeContext {
    pub(crate) fn default() -> InvokeContext {
        InvokeContext {
            boxed: None,
        }
    }

    pub fn is_some<T>(&self) -> bool
        where T: Any + Default
    {
        match &self.boxed {
            None => false,
            Some(v) => v.is::<T>(),
        }
    }

    pub fn get<T>(&self) -> Option<&T>
        where T: Any + Default
    {
        self.boxed.as_ref()
            .and_then(|boxed| boxed.downcast_ref::<T>())
    }

    pub fn get_or_default<T>(&mut self) -> &mut T
        where T: Any + Default
    {
        let is_some = self.is_some::<T>();

        if is_some {
            self.boxed
                .as_mut()
                .unwrap()
                .downcast_mut::<T>()
                .unwrap()
        } else {
            self.boxed
                .insert(Box::new(T::default()))
                .downcast_mut::<T>()
                .unwrap()
        }
    }
}
