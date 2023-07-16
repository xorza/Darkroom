use std::ops::{Index, IndexMut};

use crate::data::Value;
use crate::graph::{Binding, Graph};
use crate::invoke::Invoker;
use crate::runtime_graph::RuntimeGraph;

#[derive(Default)]
pub(crate) struct ArgSet(Vec<Option<Value>>);


pub struct Compute {
    invoker: Box<dyn Invoker>,
}

impl Compute {
    pub fn run(
        &self,
        graph: &Graph,
        runtime_graph: &mut RuntimeGraph,
    ) -> anyhow::Result<()>
    {
        let mut inputs: ArgSet = ArgSet::default();

        let active_node_indexes =
            runtime_graph.nodes
                .iter_mut()
                .enumerate()
                .filter_map(|(index, r_node)| {
                    if !r_node.has_missing_inputs && r_node.should_execute {
                        Some(index)
                    } else {
                        None
                    }
                })
                .collect::<Vec<usize>>();

        for index in active_node_indexes {
            let node = graph
                .node_by_id(runtime_graph.nodes[index].node_id()).unwrap();

            inputs.resize_and_fill(node.inputs.len());
            node.inputs
                .iter()
                .map(|input| {
                    match &input.binding {
                        Binding::None => None,
                        Binding::Const => input.const_value.clone(),

                        Binding::Output(output_binding) => {
                            let output_r_node = runtime_graph
                                .node_by_id_mut(output_binding.output_node_id).unwrap();

                            output_r_node.decrement_binding_count(output_binding.output_index);

                            let output_values =
                                output_r_node.output_values
                                    .as_mut().unwrap();
                            let value =
                                output_values
                                    .get_mut(output_binding.output_index as usize).unwrap()
                                    .clone();

                            value
                        }
                    }
                })
                .enumerate()
                .for_each(|(index, value)| {
                    inputs[index] = value
                });

            let r_node = &mut runtime_graph.nodes[index];
            let outputs =
                r_node.output_values
                    .get_or_insert_with(|| vec![None; node.outputs.len()]);

            r_node.run_time = {
                let start = std::time::Instant::now();
                self.invoker.invoke(
                    node.function_id,
                    &mut r_node.invoke_context,
                    inputs.as_slice(),
                    outputs.as_mut_slice(),
                )?;

                start.elapsed().as_secs_f64()
            };

            inputs.fill();
        }

        debug_assert!(
            runtime_graph.nodes
                .iter()
                .all(|r_node| r_node.total_binding_count == 0)
        );

        Ok(())
    }
}

impl<T: Invoker + 'static> From<T> for Compute {
    fn from(invoker: T) -> Self {
        Compute {
            invoker: Box::new(invoker),
        }
    }
}
impl From<Box<dyn Invoker>> for Compute {
    fn from(invoker: Box<dyn Invoker>) -> Self {
        Compute {
            invoker,
        }
    }
}


impl ArgSet {
    pub(crate) fn from_vec<T>(vec: Vec<Option<T>>) -> Self
    where T: Into<Value> {
        ArgSet(vec.into_iter().map(|v| v.map(|v| v.into())).collect())
    }
    pub(crate) fn resize_and_fill(&mut self, size: usize) {
        self.0.resize(size, None);
        self.fill();
    }
    pub(crate) fn fill(&mut self) {
        self.0.fill(None);
    }
    pub(crate) fn as_slice(&self) -> &[Option<Value>] {
        self.0.as_slice()
    }
    pub(crate) fn as_mut_slice(&mut self) -> &mut [Option<Value>] {
        self.0.as_mut_slice()
    }
}
impl Index<usize> for ArgSet {
    type Output = Option<Value>;

    fn index(&self, index: usize) -> &Self::Output {
        &self.0[index]
    }
}
impl IndexMut<usize> for ArgSet {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.0[index]
    }
}
