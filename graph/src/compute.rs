use std::fmt::Debug;
use std::mem::take;
use std::ops::{Index, IndexMut};

use crate::data::{DataType, DynamicValue};
use crate::execution_graph::{ExecutionBehavior, ExecutionGraph, ExecutionGraphError, InputState};
use crate::function::{FuncId, FuncLib};
use crate::graph::{Binding, Graph, NodeId};
use crate::prelude::InvokeCache;
use common::key_index_vec::{KeyIndexKey, KeyIndexVec};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::data::StaticValue;
    use crate::execution_graph::ExecutionGraph;
    use crate::function::{test_func_lib, FuncBehavior, TestFuncHooks};
    use crate::graph::{test_graph, Binding, NodeBehavior};
    use tokio::sync::Mutex;

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
            get_a: Box::new(move || test_values_a.try_lock().unwrap().a),
            get_b: Box::new(move || test_values_b.try_lock().unwrap().b),
            print: Box::new(move |result| {
                test_values_result.try_lock().unwrap().result = result;
            }),
        });

        let graph = test_graph();

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib)?;
        execution_graph.run(&graph, &func_lib)?;
        assert_eq!(test_values.try_lock()?.result, 35);

        // get_b is pure, so changing this should not affect result
        test_values.try_lock()?.b = 7;

        execution_graph.update(&graph, &func_lib)?;
        execution_graph.run(&graph, &func_lib)?;
        assert_eq!(test_values.try_lock()?.result, 35);

        func_lib.by_name_mut("get_b").unwrap().behavior = FuncBehavior::Impure;

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib)?;
        execution_graph.run(&graph, &func_lib)?;

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
                test_values_result.try_lock().unwrap().result = result;
            }),
            ..TestFuncHooks::default()
        });

        let mut graph = test_graph();

        {
            let sum_inputs = &mut graph.by_name_mut("sum").unwrap().inputs;
            sum_inputs[0].const_value = Some(StaticValue::from(29));
            sum_inputs[0].binding = Binding::Const;
            sum_inputs[1].const_value = Some(StaticValue::from(11));
            sum_inputs[1].binding = Binding::Const;
        }

        {
            let mult_inputs = &mut graph.by_name_mut("mult").unwrap().inputs;
            mult_inputs[1].const_value = Some(StaticValue::from(9));
            mult_inputs[1].binding = Binding::Const;
        }

        let mut execution_graph = ExecutionGraph::default();

        execution_graph.update(&graph, &func_lib)?;
        execution_graph.run(&graph, &func_lib)?;

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
                let mut guard = test_values_a.try_lock().unwrap();
                let a1 = guard.a;
                guard.a += 1;

                a1
            }),
            get_b: Box::new(move || {
                let mut guard = test_values_b.try_lock().unwrap();
                let b1 = guard.b;
                guard.b += 1;
                if b1 == 6 {
                    panic!("Unexpected call to get_b");
                }

                b1
            }),
            print: Box::new(move |result| {
                test_values_result.try_lock().unwrap().result = result;
            }),
        });

        let mut graph = test_graph();
        func_lib.by_name_mut("get_a").unwrap().behavior = FuncBehavior::Impure;
        graph.by_name_mut("get_a").unwrap().behavior = NodeBehavior::AsFunction;

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib)?;
        execution_graph.run(&graph, &func_lib)?;

        // assert that both nodes were called
        {
            let guard = test_values.try_lock()?;
            assert_eq!(guard.a, 3);
            assert_eq!(guard.b, 6);
            assert_eq!(guard.result, 35);
        }

        execution_graph.update(&graph, &func_lib)?;
        execution_graph.run(&graph, &func_lib)?;

        // assert that node was called again
        let guard = test_values.try_lock()?;
        assert_eq!(guard.a, 4);
        // but node b was cached
        assert_eq!(guard.b, 6);
        assert_eq!(guard.result, 40);

        Ok(())
    }
}
