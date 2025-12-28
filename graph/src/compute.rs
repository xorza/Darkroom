use std::fmt::Debug;
use std::ops::{Index, IndexMut};

use crate::data::{DataType, DynamicValue};
use crate::execution_graph::{ExecutionGraph, ExecutionGraphError};
use crate::function::{FuncId, FuncLib};
use crate::graph::{Binding, Graph};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Default)]
pub(crate) struct ArgSet(Vec<DynamicValue>);

#[derive(Debug, Default)]
pub struct Compute {}

#[derive(Debug, Error, Clone, Serialize, Deserialize)]
pub enum ComputeError {
    #[error("Execution graph update failed: {0}")]
    ExecutionGraph(#[from] ExecutionGraphError),
    #[error("Function invocation failed for function {function_id:?}: {message}")]
    Invoke {
        function_id: FuncId,
        message: String,
    },
}

type ComputeResult<T> = std::result::Result<T, ComputeError>;

impl Compute {
    pub async fn run(
        &self,
        graph: &Graph,
        func_lib: &FuncLib,
        execution_graph: &mut ExecutionGraph,
    ) -> ComputeResult<()> {
        execution_graph.update(graph, func_lib)?;
        let mut inputs: ArgSet = ArgSet::default();

        for e_node_idx in execution_graph.e_node_execution_order.iter().copied() {
            let (node, func) = {
                let e_node = &execution_graph.e_nodes[e_node_idx];
                assert!(!e_node.id.is_nil());

                if !e_node.should_invoke {
                    continue;
                }

                let node = &graph.nodes[e_node.node_idx];
                let func = &func_lib.funcs[e_node.func_idx];

                assert_eq!(
                    node.func_id, func.id,
                    "Node {:?} function ID mismatch",
                    node.id
                );
                assert_eq!(
                    node.inputs.len(),
                    func.inputs.len(),
                    "Node {:?} input count mismatch",
                    node.id
                );
                assert_eq!(
                    e_node.outputs.len(),
                    func.outputs.len(),
                    "Node {:?} output count mismatch",
                    node.id
                );
                assert_eq!(
                    e_node.inputs.len(),
                    func.inputs.len(),
                    "Node {:?} output count mismatch",
                    node.id
                );

                (node, func)
            };

            inputs.resize_and_clear(node.inputs.len());
            for (input_idx, input) in node.inputs.iter().enumerate() {
                let value: DynamicValue = match &input.binding {
                    Binding::None => DynamicValue::None,
                    Binding::Const => input
                        .const_value
                        .as_ref()
                        .expect("Const value is not set")
                        .into(),

                    Binding::Output(output_binding) => {
                        let output_address = &execution_graph.e_nodes[e_node_idx].inputs[input_idx]
                            .output_address
                            .expect("Output address is not set");
                        let output_values = execution_graph.e_nodes[output_address.e_node_idx]
                            .output_values
                            .as_mut()
                            .expect("Output values missing for bound node; check execution order");

                        output_values[output_binding.output_idx].clone()
                    }
                };

                let data_type = &func.inputs[input_idx].data_type;
                inputs[input_idx] = self.convert_type(&value, data_type);
            }

            let e_node = &mut execution_graph.e_nodes[e_node_idx];
            let outputs = e_node
                .output_values
                .get_or_insert_with(|| vec![DynamicValue::None; func.outputs.len()]);

            let start = std::time::Instant::now();
            let invoke_result = func_lib
                .invoke_by_index(
                    e_node.func_idx,
                    &mut e_node.cache,
                    inputs.as_slice(),
                    outputs.as_mut_slice(),
                )
                .map_err(|source| ComputeError::Invoke {
                    function_id: node.func_id,
                    message: source.to_string(),
                });
            e_node.run_time = start.elapsed().as_secs_f64();
            if let Err(error) = invoke_result {
                e_node.error = Some(error.clone());
                return Err(error);
            }
            e_node.error = None;

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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::sync::Mutex;

    use crate::compute::{Compute, ComputeError};
    use crate::data::StaticValue;
    use crate::execution_graph::ExecutionGraph;
    use crate::function::{test_func_lib, FuncBehavior, TestFuncHooks};
    use crate::graph::{test_graph, Binding, NodeBehavior};

    #[derive(Debug)]
    struct TestValues {
        a: i64,
        b: i64,
        result: i64,
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn simple_compute_test() -> anyhow::Result<()> {
        let test_values = Arc::new(Mutex::new(TestValues {
            a: 2,
            b: 5,
            result: 0,
        }));

        let test_values_a = test_values.clone();
        let test_values_b = test_values.clone();
        let test_values_result = test_values.clone();
        let mut func_lib = test_func_lib(TestFuncHooks {
            get_a: Box::new(move || {
                test_values_a
                    .try_lock()
                    .expect("TestValues mutex is already locked")
                    .a
            }),
            get_b: Box::new(move || {
                test_values_b
                    .try_lock()
                    .expect("TestValues mutex is already locked")
                    .b
            }),
            print: Box::new(move |result| {
                test_values_result
                    .try_lock()
                    .expect("TestValues mutex is already locked")
                    .result = result;
            }),
        });

        let graph = test_graph();

        let mut execution_graph = ExecutionGraph::default();
        Compute::default()
            .run(&graph, &func_lib, &mut execution_graph)
            .await?;
        assert_eq!(test_values.try_lock()?.result, 35);

        // get_b is pure, so changing this should not affect result
        test_values.try_lock()?.b = 7;

        Compute::default()
            .run(&graph, &func_lib, &mut execution_graph)
            .await?;
        assert_eq!(test_values.try_lock()?.result, 35);

        func_lib
            .by_name_mut("get_b")
            .expect("Func named \"get_b\" not found")
            .behavior = FuncBehavior::Impure;

        let mut execution_graph = ExecutionGraph::default();
        Compute::default()
            .run(&graph, &func_lib, &mut execution_graph)
            .await?;

        assert_eq!(test_values.try_lock()?.result, 63);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn default_input_value() -> anyhow::Result<()> {
        let test_values = Arc::new(Mutex::new(TestValues {
            a: 2,
            b: 5,
            result: 0,
        }));
        let test_values_result = test_values.clone();

        let func_lib = test_func_lib(TestFuncHooks {
            print: Box::new(move |result| {
                test_values_result
                    .try_lock()
                    .expect("TestValues mutex is already locked")
                    .result = result;
            }),
            ..TestFuncHooks::default()
        });

        let mut graph = test_graph();

        {
            let sum_inputs = &mut graph
                .by_name_mut("sum")
                .unwrap_or_else(|| panic!("Node named \"sum\" not found"))
                .inputs;
            sum_inputs[0].const_value = Some(StaticValue::from(29));
            sum_inputs[0].binding = Binding::Const;
            sum_inputs[1].const_value = Some(StaticValue::from(11));
            sum_inputs[1].binding = Binding::Const;
        }

        {
            let mult_inputs = &mut graph
                .by_name_mut("mult")
                .unwrap_or_else(|| panic!("Node named \"mult\" not found"))
                .inputs;
            mult_inputs[1].const_value = Some(StaticValue::from(9));
            mult_inputs[1].binding = Binding::Const;
        }

        let mut execution_graph = ExecutionGraph::default();

        Compute::default()
            .run(&graph, &func_lib, &mut execution_graph)
            .await?;

        assert_eq!(test_values.try_lock()?.result, 360);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cached_value() -> anyhow::Result<()> {
        let test_values = Arc::new(Mutex::new(TestValues {
            a: 2,
            b: 5,
            result: 0,
        }));

        let test_values_a = test_values.clone();
        let test_values_b = test_values.clone();
        let test_values_result = test_values.clone();
        let mut func_lib = test_func_lib(TestFuncHooks {
            get_a: Box::new(move || {
                let mut guard = test_values_a
                    .try_lock()
                    .expect("TestValues mutex is already locked");
                let a1 = guard.a;
                guard.a += 1;

                a1
            }),
            get_b: Box::new(move || {
                let mut guard = test_values_b
                    .try_lock()
                    .expect("TestValues mutex is already locked");
                let b1 = guard.b;
                guard.b += 1;
                if b1 == 6 {
                    panic!("Unexpected call to get_b");
                }

                b1
            }),
            print: Box::new(move |result| {
                test_values_result
                    .try_lock()
                    .expect("TestValues mutex is already locked")
                    .result = result;
            }),
        });

        func_lib
            .by_name_mut("get_a")
            .expect("Func named \"get_a\" not found")
            .behavior = FuncBehavior::Impure;

        let mut graph = test_graph();
        graph
            .by_name_mut("sum")
            .expect("Node named \"sum\" not found")
            .behavior = NodeBehavior::OnInputChange;

        let mut execution_graph = ExecutionGraph::default();

        Compute::default()
            .run(&graph, &func_lib, &mut execution_graph)
            .await?;

        // assert that both nodes were called
        {
            let guard = test_values.try_lock()?;
            assert_eq!(guard.a, 3);
            assert_eq!(guard.b, 6);
            assert_eq!(guard.result, 35);
        }

        Compute::default()
            .run(&graph, &func_lib, &mut execution_graph)
            .await?;

        // assert that node was called again
        let guard = test_values.try_lock()?;
        assert_eq!(guard.a, 4);
        // but node b was cached
        assert_eq!(guard.b, 6);
        assert_eq!(guard.result, 40);

        Ok(())
    }
}
