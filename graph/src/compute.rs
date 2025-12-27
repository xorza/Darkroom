use std::fmt::Debug;
use std::ops::{Index, IndexMut};

use crate::data::{DataType, DynamicValue};
use crate::function::FuncLib;
use crate::graph::{Binding, Graph};
use crate::invoke::Invoker;
use crate::runtime_graph::RuntimeGraph;

#[derive(Debug, Default)]
pub(crate) struct ArgSet(Vec<DynamicValue>);

#[derive(Debug, Default)]
pub struct Compute {}

impl Compute {
    pub async fn run<T>(
        &self,
        graph: &Graph,
        func_lib: &FuncLib,
        invoker: &T,
        runtime_graph: &mut RuntimeGraph,
    ) -> anyhow::Result<()>
    where
        T: Invoker,
    {
        runtime_graph.next(graph, func_lib);
        let graph_node_index_by_id = graph.build_node_index_by_id();

        let mut inputs: ArgSet = ArgSet::default();

        let active_node_indexes = {
            let mut active_node_indexes = runtime_graph
                .r_nodes
                .iter()
                .enumerate()
                .filter_map(|(index, r_node)| r_node.should_invoke.then_some(index))
                .collect::<Vec<usize>>();

            active_node_indexes.sort_by_key(|&index| runtime_graph.r_nodes[index].invocation_order);
            active_node_indexes
        };

        for node_idx in active_node_indexes {
            let node_id = runtime_graph.r_nodes[node_idx].id;
            let node = &graph.nodes[*graph_node_index_by_id
                .get(&node_id)
                .expect("Runtime node missing from the graph")];
            let func = func_lib
                .func_by_id(node.func_id)
                .unwrap_or_else(|| panic!("Func with id {:?} not found", node.func_id));

            assert_eq!(
                node.inputs.len(),
                func.inputs.len(),
                "Node {:?} input count mismatch",
                node.id
            );

            inputs.resize_and_clear(node.inputs.len());
            node.inputs
                .iter()
                .enumerate()
                .map(|(input_idx, input)| {
                    let value: DynamicValue = match &input.binding {
                        Binding::None => DynamicValue::None,
                        Binding::Const => input
                            .const_value
                            .as_ref()
                            .expect("Const value is not set")
                            .into(),

                        Binding::Output(output_binding) => {
                            let output_r_node = runtime_graph
                                .node_by_id_mut(output_binding.output_node_id)
                                .unwrap_or_else(|| {
                                    panic!(
                                        "Runtime node with id {:?} not found",
                                        output_binding.output_node_id
                                    )
                                });

                            let output_values = output_r_node.output_values.as_mut().expect(
                                "Output values missing for bound node; check execution order",
                            );
                            let value = output_values
                                .get_mut(output_binding.output_idx)
                                .expect("Output index out of range for bound node");

                            value.clone()
                        }
                    };

                    let data_type = &func.inputs[input_idx].data_type;

                    (input_idx, data_type, value)
                })
                .for_each(|(input_idx, data_type, value)| {
                    inputs[input_idx] = self.convert_type(&value, data_type);
                });

            let r_node = &mut runtime_graph.r_nodes[node_idx];
            let outputs = r_node
                .output_values
                .get_or_insert_with(|| vec![DynamicValue::None; func.outputs.len()]);

            r_node.run_time = {
                let start = std::time::Instant::now();
                invoker
                    .invoke(
                        node.func_id,
                        &mut r_node.cache,
                        inputs.as_mut_slice(),
                        outputs.as_mut_slice(),
                    )
                    .await?;

                start.elapsed().as_secs_f64()
            };

            inputs.clear();
        }

        Ok(())
    }

    fn convert_type(&self, src_value: &DynamicValue, dst_data_type: &DataType) -> DynamicValue {
        let src_data_type = src_value.data_type();
        if *src_data_type == *dst_data_type {
            return src_value.clone();
        }

        if src_data_type.is_custom() || dst_data_type.is_custom() {
            panic!("Custom types are not supported yet");
        }

        match (src_data_type, dst_data_type) {
            (DataType::Bool, DataType::Int) => DynamicValue::Int(src_value.as_bool() as i64),
            (DataType::Bool, DataType::Float) => {
                DynamicValue::Float(src_value.as_bool() as i64 as f64)
            }
            (DataType::Bool, DataType::String) => {
                DynamicValue::String(src_value.as_bool().to_string())
            }

            (DataType::Int, DataType::Bool) => DynamicValue::Bool(src_value.as_int() != 0),
            (DataType::Int, DataType::Float) => DynamicValue::Float(src_value.as_int() as f64),
            (DataType::Int, DataType::String) => {
                DynamicValue::String(src_value.as_int().to_string())
            }

            (DataType::Float, DataType::Bool) => {
                DynamicValue::Bool(src_value.as_float().abs() > common::EPSILON)
            }
            (DataType::Float, DataType::Int) => DynamicValue::Int(src_value.as_float() as i64),
            (DataType::Float, DataType::String) => {
                DynamicValue::String(src_value.as_float().to_string())
            }

            (src, dst) => {
                panic!("Unsupported conversion from {:?} to {:?}", src, dst);
            }
        }
    }
}

impl ArgSet {
    pub(crate) fn from_vec<T>(vec: Vec<T>) -> Self
    where
        T: Into<DynamicValue>,
    {
        ArgSet(vec.into_iter().map(|v| v.into()).collect())
    }
    pub(crate) fn resize_and_clear(&mut self, size: usize) {
        self.0.resize(size, DynamicValue::None);
        self.clear();
    }
    pub(crate) fn clear(&mut self) {
        self.0.fill(DynamicValue::None);
    }
    pub(crate) fn as_slice(&self) -> &[DynamicValue] {
        self.0.as_slice()
    }
    pub(crate) fn as_mut_slice(&mut self) -> &mut [DynamicValue] {
        self.0.as_mut_slice()
    }
}
impl Index<usize> for ArgSet {
    type Output = DynamicValue;

    fn index(&self, index: usize) -> &Self::Output {
        &self.0[index]
    }
}
impl IndexMut<usize> for ArgSet {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.0[index]
    }
}
