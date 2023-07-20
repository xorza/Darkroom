use std::ops::{Index, IndexMut};

use common::ApplyMut;

use crate::data::{DataType, DynamicValue};
use crate::graph::{Binding, Graph};
use crate::invoke::Invoker;
use crate::runtime_graph::RuntimeGraph;

#[derive(Default)]
pub(crate) struct ArgSet(Vec<Option<DynamicValue>>);

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
        runtime_graph.next(graph);

        let mut inputs: ArgSet = ArgSet::default();

        let active_node_indexes =
            runtime_graph.nodes
                .iter_mut()
                .enumerate()
                .filter_map(|(index, r_node)| {
                    if r_node.should_invoke {
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
                    let value = match &input.binding {
                        Binding::None => None,
                        Binding::Const => input.const_value
                            .as_ref()
                            .map(DynamicValue::from),

                        Binding::Output(output_binding) => {
                            let output_r_node = runtime_graph
                                .node_by_id_mut(output_binding.output_node_id).unwrap();

                            output_r_node.decrement_current_binding_count(output_binding.output_index);

                            let output_values =
                                output_r_node.output_values
                                    .as_mut().unwrap();
                            let value =
                                output_values
                                    .get_mut(output_binding.output_index as usize).unwrap();

                            value.clone()
                        }
                    };
                    let data_type = &input.data_type;

                    (data_type, value)
                })
                .enumerate()
                .for_each(|(index, (data_type, value))| {
                    inputs[index] = value
                        .as_ref()
                        .map(|value| {
                            self.convert_type(value, data_type)
                        });
                });

            let r_node = &mut runtime_graph.nodes[index];
            let outputs =
                r_node.output_values
                    .get_or_insert_with(|| vec![None; node.outputs.len()]);
            assert_eq!(outputs.len(), node.outputs.len());

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

        for r_node in runtime_graph.nodes.iter_mut() {
            if !r_node.should_cache_outputs {
                r_node.output_values
                    .as_mut()
                    .apply_mut(|values| values.fill(None));
            }

            assert_eq!(r_node.total_binding_count, 0);
        }

        Ok(())
    }

    fn convert_type(&self,
                    src_value: &DynamicValue,
                    dst_data_type: &DataType)
        -> DynamicValue
    {
        let src_data_type = src_value.data_type();
        if *src_data_type == *dst_data_type {
            return src_value.clone();
        }

        if src_data_type.is_custom() || dst_data_type.is_custom() {
            return self.invoker.convert_value(src_value, dst_data_type);
        }

        match (src_data_type, dst_data_type) {
            (DataType::Bool, DataType::Int) => DynamicValue::Int(src_value.as_bool() as i64),
            (DataType::Bool, DataType::Float) => DynamicValue::Float(src_value.as_bool() as i64 as f64),
            (DataType::Bool, DataType::String) => DynamicValue::String(src_value.as_bool().to_string()),

            (DataType::Int, DataType::Bool) => DynamicValue::Bool(src_value.as_int() != 0),
            (DataType::Int, DataType::Float) => DynamicValue::Float(src_value.as_int() as f64),
            (DataType::Int, DataType::String) => DynamicValue::String(src_value.as_int().to_string()),

            (DataType::Float, DataType::Bool) => DynamicValue::Bool(src_value.as_float().abs() > common::EPSILON),
            (DataType::Float, DataType::Int) => DynamicValue::Int(src_value.as_float() as i64),
            (DataType::Float, DataType::String) => DynamicValue::String(src_value.as_float().to_string()),

            (src, dst) => {
                panic!("Unsupported conversion from {:?} to {:?}", src, dst);
            }
        }
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
    where T: Into<DynamicValue> {
        ArgSet(vec.into_iter().map(|v| v.map(|v| v.into())).collect())
    }
    pub(crate) fn resize_and_fill(&mut self, size: usize) {
        self.0.resize(size, None);
        self.fill();
    }
    pub(crate) fn fill(&mut self) {
        self.0.fill(None);
    }
    pub(crate) fn as_slice(&self) -> &[Option<DynamicValue>] {
        self.0.as_slice()
    }
    pub(crate) fn as_mut_slice(&mut self) -> &mut [Option<DynamicValue>] {
        self.0.as_mut_slice()
    }
}
impl Index<usize> for ArgSet {
    type Output = Option<DynamicValue>;

    fn index(&self, index: usize) -> &Self::Output {
        &self.0[index]
    }
}
impl IndexMut<usize> for ArgSet {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.0[index]
    }
}
