use std::any::Any;
use std::collections::HashMap;
use std::ops::{Index, IndexMut};

use crate::data::Value;
use crate::functions::FunctionId;
use crate::graph::{Binding, FunctionBehavior, Graph, NodeId};
use crate::preprocess::PreprocessInfo;

pub(crate) type InvokeArgs = [Option<Value>];

#[derive(Default, Debug)]
pub(crate) struct ArgSet {
    args: Vec<Option<Value>>,
    binding_count: Vec<u32>,
}

#[derive(Debug, Default)]
pub struct InvokeContext {
    boxed: Option<Box<dyn Any>>,
}

#[derive(Default, Debug)]
pub struct ComputeCache {
    output_args: HashMap<NodeId, ArgSet>,
    contexts: HashMap<NodeId, InvokeContext>,
}

pub struct NodeInvokeInfo {
    node_id: NodeId,
    runtime: f64,
}

#[derive(Default)]
pub struct ComputeInfo {
    node_invoke_infos: Vec<NodeInvokeInfo>,
}

pub trait Invokable {
    fn call(&self, ctx: &mut InvokeContext, inputs: &InvokeArgs, outputs: &mut InvokeArgs);
}

pub trait Compute {
    fn run(&self,
           graph: &Graph,
           preprocess_info: &PreprocessInfo,
           compute_cache: &mut ComputeCache, )
        -> anyhow::Result<ComputeInfo>
    {
        let mut compute_info = ComputeInfo::default();
        let mut inputs: ArgSet = ArgSet::default();

        let mut empty_context = InvokeContext::default();

        for pp_node in preprocess_info.nodes.iter() {
            let node = graph.node_by_id(pp_node.node_id()).unwrap();

            inputs.resize_and_fill(node.inputs.len());
            node.inputs.iter()
                .map(|input| {
                    match &input.binding {
                        Binding::None =>
                            None,

                        Binding::Const =>
                            input.const_value.clone(),

                        Binding::Output(output_binding) => {
                            let args = compute_cache.output_args
                                .get_mut(&output_binding.output_node_id)
                                .unwrap();

                            let binding_count = &mut args.binding_count[output_binding.output_index as usize];
                            *binding_count -= 1;

                            if *binding_count == 0 && pp_node.behavior == FunctionBehavior::Active {
                                args[output_binding.output_index as usize]
                                    .take()
                            } else {
                                args[output_binding.output_index as usize]
                                    .clone()
                            }
                        }
                    }
                })
                .enumerate()
                .for_each(|(index, value)| {
                    inputs[index] = value
                });

            let outputs = compute_cache.output_args
                .entry(node.id())
                .or_insert_with(|| ArgSet::with_size(node.outputs.len() as u32));
            pp_node.outputs
                .iter()
                .enumerate()
                .for_each(|(index, output)| {
                    outputs.binding_count[index] = output.binding_count;
                });

            {
                let ctx = compute_cache.contexts
                    .get_mut(&node.id())
                    .unwrap_or(&mut empty_context);

                let start = std::time::Instant::now();
                self.invoke(node.function_id, ctx, inputs.as_slice(), outputs.as_mut_slice())?;
                let elapsed = start.elapsed();

                compute_info.node_invoke_infos.push(NodeInvokeInfo {
                    node_id: node.id(),
                    runtime: elapsed.as_secs_f64(),
                });
            }

            if !empty_context.is_none() {
                compute_cache.contexts.insert(node.id(), std::mem::take(&mut empty_context));
            }


            inputs.resize_and_fill(0);
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

#[derive(Default)]
pub struct LambdaCompute {
    lambdas: HashMap<FunctionId, LambdaInvokable>,
}

impl LambdaCompute {
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
    pub fn with_size(size: u32) -> Self {
        Self {
            args: vec![None; size as usize],
            binding_count: vec![0; size as usize],
        }
    }
    pub fn from_vec<T, V>(args: Vec<T>) -> Self
    where T: Into<Option<V>>,
          V: Into<Value>
    {
        let count = args.len();
        let args = args.into_iter().map(|v| v.into().map(|v| v.into())).collect();
        Self {
            args,
            binding_count: vec![0; count],
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
        self.binding_count.resize(size, 0);
        self.binding_count.fill(0);
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

    pub fn is_none(&self) -> bool {
        self.boxed.is_none()
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

    pub fn get_mut<T>(&mut self) -> Option<&mut T>
    where T: Any + Default
    {
        self.boxed.as_mut()
            .and_then(|boxed| boxed.downcast_mut::<T>())
    }

    pub fn set<T>(&mut self, value: T)
    where T: Any + Default
    {
        self.boxed = Some(Box::new(value));
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
                .insert(Box::<T>::default())
                .downcast_mut::<T>()
                .unwrap()
        }
    }
}
