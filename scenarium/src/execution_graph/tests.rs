use std::sync::Arc;

use super::*;
use crate::data::{DynamicValue, StaticValue};
use crate::function::{TestFuncHooks, test_func_lib};
use crate::graph::{NodeBehavior, test_graph};
use common::{FloatExt, SerdeFormat};
use tokio::sync::Mutex;

fn execution_node_names_in_order(execution_graph: &ExecutionGraph) -> Vec<String> {
    execution_graph
        .e_node_execute_order
        .iter()
        .map(|&e_node_idx| execution_graph.e_nodes[e_node_idx].name.clone())
        .collect()
}

#[test]
fn basic_run() -> anyhow::Result<()> {
    let graph = test_graph();
    let func_lib = test_func_lib(TestFuncHooks::default());

    let mut execution_graph = ExecutionGraph::default();
    execution_graph.update(&graph, &func_lib);
    execution_graph.prepare_execution(true, false, &[])?;

    assert_eq!(
        execution_node_names_in_order(&execution_graph)[2..],
        ["sum", "mult", "print"]
    );

    assert_eq!(execution_graph.e_nodes.len(), 5);
    assert_eq!(execution_graph.e_node_process_order.len(), 5);
    assert_eq!(execution_graph.e_node_execute_order.len(), 5);
    assert!(
        execution_graph
            .e_nodes
            .iter()
            .all(|e_node| !e_node.missing_required_inputs)
    );
    assert!(
        execution_graph
            .e_nodes
            .iter()
            .all(|e_node| e_node.wants_execute)
    );

    let get_a = execution_graph.by_name("get_a").unwrap();
    let get_b = execution_graph.by_name("get_b").unwrap();
    let sum = execution_graph.by_name("sum").unwrap();
    let mult = execution_graph.by_name("mult").unwrap();
    let print = execution_graph.by_name("print").unwrap();

    assert_eq!(get_a.outputs[0].usage_count, 1);
    assert_eq!(get_b.outputs[0].usage_count, 2);
    assert_eq!(sum.outputs[0].usage_count, 1);
    assert_eq!(mult.outputs[0].usage_count, 1);

    assert!(!get_a.inputs_updated);
    assert!(!get_b.inputs_updated);
    assert!(sum.inputs_updated);
    assert!(mult.inputs_updated);
    assert!(print.inputs_updated);

    assert!(print.terminal);

    Ok(())
}

#[test]
fn missing_input() -> anyhow::Result<()> {
    let mut graph = test_graph();
    let func_lib = test_func_lib(TestFuncHooks::default());

    graph.by_name_mut("sum").unwrap().inputs[0].binding = Binding::None;

    let mut execution_graph = ExecutionGraph::default();
    execution_graph.update(&graph, &func_lib);
    execution_graph.prepare_execution(true, false, &[])?;

    let get_b = execution_graph.by_name("get_b").unwrap();
    let sum = execution_graph.by_name("sum").unwrap();
    let mult = execution_graph.by_name("mult").unwrap();
    let print = execution_graph.by_name("print").unwrap();

    assert!(!get_b.missing_required_inputs);
    assert!(sum.missing_required_inputs);
    assert!(mult.missing_required_inputs);
    assert!(print.missing_required_inputs);

    assert_eq!(execution_graph.e_node_execute_order.len(), 0);

    Ok(())
}

#[test]
fn missing_non_required_input() -> anyhow::Result<()> {
    let mut graph = test_graph();
    let mut func_lib = test_func_lib(TestFuncHooks::default());
    let mut execution_graph = ExecutionGraph::default();

    graph.by_name_mut("sum").unwrap().inputs[0].binding = Binding::None;
    func_lib.by_name_mut("mult").unwrap().inputs[0].required = false;

    execution_graph.update(&graph, &func_lib);
    execution_graph.prepare_execution(true, false, &[])?;

    let sum = execution_graph.by_name("sum").unwrap();
    let mult = execution_graph.by_name("mult").unwrap();
    let print = execution_graph.by_name("print").unwrap();

    assert!(sum.missing_required_inputs);
    assert!(!mult.missing_required_inputs);
    assert!(!print.missing_required_inputs);

    assert_eq!(
        execution_node_names_in_order(&execution_graph),
        ["get_b", "mult", "print"]
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn const_binding() -> anyhow::Result<()> {
    let mut graph = test_graph();
    let func_lib = test_func_lib(TestFuncHooks {
        get_a: Arc::new(move || Ok(1)),
        get_b: Arc::new(move || 11),
        print: Arc::new(move |_| {}),
    });
    let mut execution_graph = ExecutionGraph::default();

    let mult = graph.by_name_mut("mult").unwrap();
    mult.inputs[0].binding = Binding::Const(StaticValue::Int(3));
    mult.inputs[1].binding = Binding::Const(StaticValue::Int(5));

    execution_graph.update(&graph, &func_lib);

    let mult = execution_graph.by_name("mult").unwrap();
    assert!(mult.inputs[0].binding_changed);
    assert!(mult.inputs[1].binding_changed);

    execution_graph.execute_terminals().await?;

    assert_eq!(
        execution_node_names_in_order(&execution_graph),
        ["mult", "print"]
    );

    let mult = execution_graph.by_name("mult").unwrap();
    assert!(mult.inputs_updated);
    assert!(!mult.inputs[0].binding_changed);
    assert!(!mult.inputs[0].dependency_wants_execute);
    assert!(!mult.inputs[1].binding_changed);
    assert!(!mult.inputs[1].dependency_wants_execute);

    execution_graph.update(&graph, &func_lib);
    execution_graph.execute_terminals().await?;

    assert_eq!(execution_node_names_in_order(&execution_graph), ["print"]);

    let mult = execution_graph.by_name("mult").unwrap();
    assert!(!mult.inputs_updated);
    assert!(!mult.inputs[0].binding_changed);
    assert!(!mult.inputs[1].binding_changed);

    graph.by_name_mut("mult").unwrap().inputs[0].binding = Binding::Const(StaticValue::Int(4));
    execution_graph.update(&graph, &func_lib);
    execution_graph.prepare_execution(true, false, &[])?;

    let mult = execution_graph.by_name("mult").unwrap();
    let print = execution_graph.by_name("print").unwrap();

    assert_eq!(
        execution_node_names_in_order(&execution_graph),
        ["mult", "print"]
    );

    assert!(mult.inputs[0].binding_changed);
    assert!(!mult.inputs[1].binding_changed);
    assert!(!mult.missing_required_inputs);
    assert!(!print.missing_required_inputs);
    assert!(mult.inputs_updated);
    assert!(print.inputs_updated);

    Ok(())
}

#[test]
fn roundtrip_serialization() -> anyhow::Result<()> {
    let graph = test_graph();
    let func_lib = test_func_lib(TestFuncHooks::default());

    let mut execution_graph = ExecutionGraph::default();
    execution_graph.update(&graph, &func_lib);

    for format in SerdeFormat::all_formats_for_testing() {
        let serialized = execution_graph.serialize(format);
        let deserialized = ExecutionGraph::deserialize(&serialized, format)?;
        let serialized_again = deserialized.serialize(format);
        assert_eq!(serialized, serialized_again);
    }

    Ok(())
}

#[test]
fn execution_graph_updates_after_graph_change() -> anyhow::Result<()> {
    let mut graph = test_graph();
    let func_lib = test_func_lib(TestFuncHooks::default());
    let mut execution_graph = ExecutionGraph::default();

    execution_graph.update(&graph, &func_lib);

    let binding1: Binding = (graph.by_name("get_a").unwrap().id, 0).into();
    let binding2: Binding = (graph.by_name("get_b").unwrap().id, 0).into();
    let mult = graph.by_name_mut("mult").unwrap();
    mult.inputs[0].binding = binding1;
    mult.inputs[1].binding = binding2;

    execution_graph.update(&graph, &func_lib);
    execution_graph.prepare_execution(true, false, &[])?;

    let get_a = execution_graph.by_name("get_a").unwrap();
    let get_b = execution_graph.by_name("get_b").unwrap();
    let mult = execution_graph.by_name("mult").unwrap();
    let print = execution_graph.by_name("print").unwrap();

    assert_eq!(get_a.outputs.len(), 1);
    assert_eq!(get_b.outputs.len(), 1);
    assert_eq!(mult.outputs.len(), 1);
    assert!(print.outputs.is_empty());
    assert_eq!(get_a.outputs[0].usage_count, 1);
    assert_eq!(get_b.outputs[0].usage_count, 1);
    assert_eq!(mult.outputs[0].usage_count, 1);

    Ok(())
}

#[test]
fn pure_node_skips_consequent_invokations() -> anyhow::Result<()> {
    let mut graph = test_graph();
    let mut func_lib = test_func_lib(TestFuncHooks::default());

    graph.by_name_mut("get_b").unwrap().behavior = NodeBehavior::AsFunction;
    func_lib.by_name_mut("get_b").unwrap().behavior = FuncBehavior::Pure;

    let mut execution_graph = ExecutionGraph::default();
    execution_graph.update(&graph, &func_lib);
    execution_graph.prepare_execution(true, false, &[])?;

    assert!(execution_node_names_in_order(&execution_graph).contains(&"get_b".to_string()));

    execution_graph.by_name_mut("get_b").unwrap().output_values = Some(vec![DynamicValue::Int(7)]);

    execution_graph.update(&graph, &func_lib);
    execution_graph.prepare_execution(true, false, &[])?;

    assert!(!execution_node_names_in_order(&execution_graph).contains(&"get_b".to_string()));

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn node_skips_consequent_invokations() -> anyhow::Result<()> {
    let graph = test_graph();
    let func_lib = test_func_lib(TestFuncHooks {
        get_a: Arc::new(move || Ok(1)),
        get_b: Arc::new(move || 11),
        print: Arc::new(move |_| {}),
    });
    let mut execution_graph = ExecutionGraph::default();

    execution_graph.update(&graph, &func_lib);
    execution_graph.execute_terminals().await?;

    assert_eq!(
        execution_node_names_in_order(&execution_graph)[2..],
        ["sum", "mult", "print"]
    );

    let exe_stats = execution_graph.execute_terminals().await?;
    assert_eq!(execution_node_names_in_order(&execution_graph), ["print"]);
    assert_eq!(exe_stats.cached_nodes.len(), 4);

    Ok(())
}

#[test]
fn inpure_node_always_invoked() -> anyhow::Result<()> {
    let mut graph = test_graph();
    let mut func_lib = test_func_lib(TestFuncHooks::default());

    graph.by_name_mut("get_b").unwrap().behavior = NodeBehavior::AsFunction;
    func_lib.by_name_mut("get_b").unwrap().behavior = FuncBehavior::Impure;

    let mut execution_graph = ExecutionGraph::default();
    execution_graph.update(&graph, &func_lib);

    execution_graph.by_name_mut("get_b").unwrap().output_values = Some(vec![DynamicValue::Int(7)]);
    execution_graph.update(&graph, &func_lib);
    execution_graph.prepare_execution(true, false, &[])?;

    assert_eq!(
        execution_node_names_in_order(&execution_graph)[2..],
        ["sum", "mult", "print"]
    );

    Ok(())
}

#[test]
fn once_node_always_caches() -> anyhow::Result<()> {
    let mut graph = test_graph();
    let mut func_lib = test_func_lib(TestFuncHooks::default());
    let mut execution_graph = ExecutionGraph::default();

    graph.by_name_mut("get_b").unwrap().behavior = NodeBehavior::Once;
    func_lib.by_name_mut("get_b").unwrap().behavior = FuncBehavior::Impure;

    execution_graph.update(&graph, &func_lib);
    execution_graph.prepare_execution(true, false, &[])?;

    assert_eq!(
        execution_node_names_in_order(&execution_graph)[2..],
        ["sum", "mult", "print"]
    );

    execution_graph.by_name_mut("get_b").unwrap().output_values = Some(vec![DynamicValue::Int(7)]);
    execution_graph.update(&graph, &func_lib);
    execution_graph.prepare_execution(true, false, &[])?;

    assert_eq!(
        execution_node_names_in_order(&execution_graph),
        ["get_a", "sum", "mult", "print"]
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn once_node_recomputes_on_binding_change() -> anyhow::Result<()> {
    let mut graph = test_graph();
    let func_lib = test_func_lib(TestFuncHooks {
        get_a: Arc::new(move || Ok(3)),
        get_b: Arc::new(move || 55),
        print: Arc::new(move |_| {}),
    });
    let mut execution_graph = ExecutionGraph::default();

    graph.by_name_mut("mult").unwrap().behavior = NodeBehavior::Once;

    execution_graph.update(&graph, &func_lib);
    execution_graph.execute_terminals().await?;

    assert_eq!(
        execution_node_names_in_order(&execution_graph)[2..],
        ["sum", "mult", "print"]
    );

    execution_graph.update(&graph, &func_lib);
    execution_graph.execute_terminals().await?;

    assert_eq!(execution_node_names_in_order(&execution_graph), ["print"]);

    let mult = graph.by_name_mut("mult").unwrap();
    mult.inputs[0].binding = mult.inputs[1].binding.clone();

    execution_graph.update(&graph, &func_lib);
    execution_graph.execute_terminals().await?;

    assert_eq!(
        execution_node_names_in_order(&execution_graph),
        ["mult", "print"]
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn once_node_recomputes_on_binding_change_with_cached_inputs() -> anyhow::Result<()> {
    let mut graph = test_graph();
    let func_lib = test_func_lib(TestFuncHooks {
        get_a: Arc::new(move || Ok(3)),
        get_b: Arc::new(move || 55),
        print: Arc::new(move |_| {}),
    });
    let mut execution_graph = ExecutionGraph::default();

    graph.by_name_mut("mult").unwrap().behavior = NodeBehavior::Once;

    execution_graph.update(&graph, &func_lib);
    execution_graph.execute_terminals().await?;

    assert_eq!(
        execution_node_names_in_order(&execution_graph)[2..],
        ["sum", "mult", "print"]
    );

    execution_graph.update(&graph, &func_lib);
    execution_graph.execute_terminals().await?;

    assert_eq!(execution_node_names_in_order(&execution_graph), ["print"]);

    let mult = graph.by_name_mut("mult").unwrap();
    let old_binding0 = mult.inputs[0].binding.clone();
    let old_binding1 = mult.inputs[1].binding.clone();
    mult.inputs[0].binding = Binding::Const(2.into());
    mult.inputs[1].binding = Binding::Const(22.into());

    execution_graph.update(&graph, &func_lib);
    execution_graph.execute_terminals().await?;

    assert_eq!(
        execution_node_names_in_order(&execution_graph),
        ["mult", "print"]
    );

    let mult = graph.by_name_mut("mult").unwrap();
    mult.inputs[0].binding = old_binding1;
    mult.inputs[1].binding = old_binding0;

    execution_graph.update(&graph, &func_lib);
    execution_graph.execute_terminals().await?;

    assert_eq!(
        execution_node_names_in_order(&execution_graph),
        ["mult", "print"],
    );

    Ok(())
}

#[test]
fn cycle_detection_returns_error() {
    let mut graph = test_graph();
    let func_lib = test_func_lib(TestFuncHooks::default());

    let mult_node_id = graph.by_name("mult").unwrap().id;
    let sum_inputs = &mut graph.by_name_mut("sum").unwrap().inputs;
    sum_inputs[0].binding = (mult_node_id, 0).into();

    let mut execution_graph = ExecutionGraph::default();
    execution_graph.update(&graph, &func_lib);

    let err = execution_graph
        .prepare_execution(true, false, &[])
        .expect_err("Expected cycle detection error");
    match err {
        Error::CycleDetected { node_id } => {
            assert_eq!(node_id, "579ae1d6-10a3-4906-8948-135cb7d7508b".into());
        }
        _ => panic!("Unexpected error"),
    }
}

#[test]
fn invalidate_recursively_marks_dependents() -> anyhow::Result<()> {
    let graph = test_graph();
    let func_lib = test_func_lib(TestFuncHooks::default());

    let mut execution_graph = ExecutionGraph::default();
    execution_graph.update(&graph, &func_lib);

    let sum = graph.by_name("sum").unwrap().id;

    execution_graph.invalidate_recursively(vec![sum]);

    Ok(())
}

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
        get_a: Arc::new(move || Ok(test_values_a.try_lock().unwrap().a)),
        get_b: Arc::new(move || test_values_b.try_lock().unwrap().b),
        print: Arc::new(move |result| {
            test_values_result.try_lock().unwrap().result = result;
        }),
    });

    let graph = test_graph();

    let mut execution_graph = ExecutionGraph::default();
    execution_graph.update(&graph, &func_lib);
    execution_graph.execute_terminals().await?;
    assert_eq!(test_values.try_lock()?.result, 35);

    test_values.try_lock()?.b = 7;

    execution_graph.update(&graph, &func_lib);
    execution_graph.execute_terminals().await?;
    assert_eq!(test_values.try_lock()?.result, 35);

    func_lib.by_name_mut("get_b").unwrap().behavior = FuncBehavior::Impure;

    let mut execution_graph = ExecutionGraph::default();
    execution_graph.update(&graph, &func_lib);
    execution_graph.execute_terminals().await?;

    assert_eq!(test_values.try_lock()?.result, 63);

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn const_binding_invokes_only_once() -> anyhow::Result<()> {
    let func_lib = test_func_lib(TestFuncHooks {
        get_a: Arc::new(move || unreachable!()),
        get_b: Arc::new(move || unreachable!()),
        print: Arc::new(move |_| {}),
    });

    let mut graph = test_graph();
    let mut execution_graph = ExecutionGraph::default();

    let mult = graph.by_name_mut("mult").unwrap();
    mult.inputs[0].binding = Binding::Const(StaticValue::Int(3));
    mult.inputs[1].binding = Binding::Const(StaticValue::Int(5));

    execution_graph.update(&graph, &func_lib);
    execution_graph.execute_terminals().await?;

    assert_eq!(
        execution_node_names_in_order(&execution_graph),
        ["mult", "print"]
    );

    graph.by_name_mut("mult").unwrap().inputs[0].binding = Binding::Const(StaticValue::Int(3));
    execution_graph.update(&graph, &func_lib);
    execution_graph.execute_terminals().await?;

    assert_eq!(execution_node_names_in_order(&execution_graph), ["print"]);

    let mult = graph.by_name_mut("mult").unwrap();
    mult.inputs[0].binding = Binding::Const(StaticValue::Int(4));
    execution_graph.update(&graph, &func_lib);
    execution_graph.execute_terminals().await?;

    assert_eq!(
        execution_node_names_in_order(&execution_graph),
        ["mult", "print"]
    );

    execution_graph.update(&graph, &func_lib);
    execution_graph.execute_terminals().await?;

    assert_eq!(execution_node_names_in_order(&execution_graph), ["print"]);

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn optional_input_binding_change_recomputes() -> anyhow::Result<()> {
    let func_lib = test_func_lib(TestFuncHooks {
        get_a: Arc::new(move || Ok(1)),
        get_b: Arc::new(move || 11),
        print: Arc::new(move |_| {}),
    });

    let mut graph = test_graph();
    let mut execution_graph = ExecutionGraph::default();

    execution_graph.update(&graph, &func_lib);
    execution_graph.execute_terminals().await?;

    let sum = graph.by_name_mut("mult").unwrap();
    sum.inputs[0].binding = Binding::Const(2.into());
    sum.inputs[1].binding = Binding::None;

    execution_graph.update(&graph, &func_lib);
    execution_graph.execute_terminals().await?;

    assert_eq!(
        execution_node_names_in_order(&execution_graph),
        ["mult", "print"]
    );

    execution_graph.update(&graph, &func_lib);
    execution_graph.execute_terminals().await?;

    assert_eq!(execution_node_names_in_order(&execution_graph), ["print"]);

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn const_input_excludes_upstream_node() -> anyhow::Result<()> {
    let func_lib = test_func_lib(TestFuncHooks {
        get_a: Arc::new(move || Ok(1)),
        get_b: Arc::new(move || 11),
        print: Arc::new(move |_| {}),
    });

    let mut graph = test_graph();
    let mut execution_graph = ExecutionGraph::default();

    let sum = graph.by_name_mut("sum").unwrap();
    sum.inputs[0].binding = Binding::Const(33.into());

    execution_graph.update(&graph, &func_lib);
    execution_graph.execute_terminals().await?;

    assert_eq!(
        execution_node_names_in_order(&execution_graph),
        ["get_b", "sum", "mult", "print"]
    );

    let sum = graph.by_name_mut("sum").unwrap();
    sum.inputs[1].binding = Binding::None;

    execution_graph.update(&graph, &func_lib);
    execution_graph.execute_terminals().await?;

    assert_eq!(
        execution_node_names_in_order(&execution_graph),
        ["sum", "mult", "print"]
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn change_from_const_to_bind_recomputes() -> anyhow::Result<()> {
    let func_lib = test_func_lib(TestFuncHooks {
        get_a: Arc::new(move || Ok(1)),
        get_b: Arc::new(move || 11),
        print: Arc::new(move |_| {}),
    });

    let mut graph = test_graph();
    let mut execution_graph = ExecutionGraph::default();

    let get_b_id = graph.by_name_mut("get_b").unwrap().id;
    let sum = graph.by_name_mut("sum").unwrap();
    sum.inputs[0].binding = Binding::Const(33.into());

    execution_graph.update(&graph, &func_lib);
    execution_graph.execute_terminals().await?;

    assert_eq!(
        execution_node_names_in_order(&execution_graph),
        ["get_b", "sum", "mult", "print"]
    );

    let sum = graph.by_name_mut("sum").unwrap();
    sum.inputs[0].binding = (get_b_id, 0).into();

    execution_graph.update(&graph, &func_lib);
    execution_graph.execute_terminals().await?;

    assert_eq!(
        execution_node_names_in_order(&execution_graph),
        ["sum", "mult", "print"]
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn required_none_binding_execute_is_stable() -> anyhow::Result<()> {
    let func_lib = test_func_lib(TestFuncHooks {
        get_a: Arc::new(move || Ok(1)),
        get_b: Arc::new(move || 11),
        print: Arc::new(move |_| {}),
    });

    let mut graph = test_graph();
    let mut execution_graph = ExecutionGraph::default();

    let sum = graph.by_name_mut("sum").unwrap();
    sum.inputs[0].binding = Binding::None;

    execution_graph.update(&graph, &func_lib);
    execution_graph.execute_terminals().await?;
    execution_graph.execute_terminals().await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn cached_upstream_output_reused_after_rebinding() -> anyhow::Result<()> {
    let func_lib = test_func_lib(TestFuncHooks {
        get_a: Arc::new(move || Ok(1)),
        get_b: Arc::new(move || 11),
        print: Arc::new(move |_| {}),
    });

    let mut graph = test_graph();
    let mut execution_graph = ExecutionGraph::default();

    execution_graph.update(&graph, &func_lib);
    execution_graph.execute_terminals().await?;

    let mult = graph.by_name_mut("mult").unwrap();
    mult.inputs[0].binding = Binding::Const(2.into());
    mult.inputs[1].binding = Binding::Const(21.into());

    execution_graph.update(&graph, &func_lib);
    execution_graph.execute_terminals().await?;

    assert_eq!(
        execution_node_names_in_order(&execution_graph),
        ["mult", "print"]
    );

    let get_b_id = graph.by_name_mut("get_b").unwrap().id;
    let mult = graph.by_name_mut("mult").unwrap();
    mult.inputs[0].binding = (get_b_id, 0).into();

    execution_graph.update(&graph, &func_lib);
    execution_graph.execute_terminals().await?;

    assert_eq!(
        execution_node_names_in_order(&execution_graph),
        ["mult", "print"]
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn once_node_toggle_refreshes_upstream_execution() -> anyhow::Result<()> {
    let func_lib = test_func_lib(TestFuncHooks {
        get_a: Arc::new(move || Ok(1)),
        get_b: Arc::new(move || 11),
        print: Arc::new(move |_| {}),
    });

    let mut graph = test_graph();
    let mut execution_graph = ExecutionGraph::default();

    let sum = graph.by_name_mut("sum").unwrap();
    sum.inputs[0].binding = Binding::Const(2.into());
    sum.inputs[1].binding = Binding::Const(21.into());

    execution_graph.update(&graph, &func_lib);
    execution_graph.execute_terminals().await?;

    let sum = graph.by_name_mut("sum").unwrap();
    sum.inputs[0].binding = Binding::Const(12.into());
    let mult = graph.by_name_mut("mult").unwrap();
    mult.behavior = NodeBehavior::Once;

    execution_graph.update(&graph, &func_lib);
    execution_graph.execute_terminals().await?;

    let sum = execution_graph.by_name("sum").unwrap();
    assert!(!sum.cached);

    assert_eq!(execution_node_names_in_order(&execution_graph), ["print"]);

    let mult = graph.by_name_mut("mult").unwrap();
    mult.behavior = NodeBehavior::AsFunction;

    execution_graph.update(&graph, &func_lib);
    execution_graph.execute_terminals().await?;

    assert_eq!(
        execution_node_names_in_order(&execution_graph),
        ["sum", "mult", "print"]
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn get_input_output_values_returns_none_for_nonexistent_node() -> anyhow::Result<()> {
    let graph = test_graph();
    let func_lib = test_func_lib(TestFuncHooks::default());
    let mut execution_graph = ExecutionGraph::default();

    execution_graph.update(&graph, &func_lib);

    let nonexistent_id: NodeId = "00000000-0000-0000-0000-000000000000".into();
    assert!(
        execution_graph
            .get_argument_values(&nonexistent_id)
            .is_none()
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn get_input_output_values_with_const_bindings() -> anyhow::Result<()> {
    let func_lib = test_func_lib(TestFuncHooks {
        get_a: Arc::new(move || unreachable!()),
        get_b: Arc::new(move || unreachable!()),
        print: Arc::new(move |_| {}),
    });

    let mut graph = test_graph();
    let mut execution_graph = ExecutionGraph::default();

    let mult = graph.by_name_mut("mult").unwrap();
    mult.inputs[0].binding = Binding::Const(StaticValue::Int(3));
    mult.inputs[1].binding = Binding::Const(StaticValue::Int(5));
    let mult_id = mult.id;

    execution_graph.update(&graph, &func_lib);
    execution_graph.execute_terminals().await?;

    let values = execution_graph.get_argument_values(&mult_id).unwrap();

    assert_eq!(values.inputs.len(), 2);
    assert!(matches!(values.inputs[0], Some(DynamicValue::Int(3))));
    assert!(matches!(values.inputs[1], Some(DynamicValue::Int(5))));

    assert_eq!(values.outputs.len(), 1);
    assert!(matches!(values.outputs[0], DynamicValue::Int(15)));

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn get_input_output_values_with_bound_outputs() -> anyhow::Result<()> {
    let func_lib = test_func_lib(TestFuncHooks {
        get_a: Arc::new(move || Ok(2)),
        get_b: Arc::new(move || 5),
        print: Arc::new(move |_| {}),
    });

    let graph = test_graph();
    let mut execution_graph = ExecutionGraph::default();

    execution_graph.update(&graph, &func_lib);
    execution_graph.execute_terminals().await?;

    let sum_id = graph.by_name("sum").unwrap().id;
    let values = execution_graph.get_argument_values(&sum_id).unwrap();

    assert_eq!(values.inputs.len(), 2);
    assert!(matches!(values.inputs[0], Some(DynamicValue::Float(v)) if v.approximately_eq(2.0)));
    assert!(matches!(values.inputs[1], Some(DynamicValue::Float(v)) if v.approximately_eq(5.0)));

    assert_eq!(values.outputs.len(), 1);
    assert!(matches!(values.outputs[0], DynamicValue::Int(7)));

    let mult_id = graph.by_name("mult").unwrap().id;
    let values = execution_graph.get_argument_values(&mult_id).unwrap();

    assert_eq!(values.inputs.len(), 2);
    assert!(matches!(values.inputs[0], Some(DynamicValue::Int(7))));
    assert!(matches!(values.inputs[1], Some(DynamicValue::Float(v)) if v.approximately_eq(5.0)));

    assert_eq!(values.outputs.len(), 1);
    assert!(matches!(values.outputs[0], DynamicValue::Int(35)));

    let print_id = graph.by_name("print").unwrap().id;
    let values = execution_graph.get_argument_values(&print_id).unwrap();

    assert_eq!(values.inputs.len(), 1);
    assert!(matches!(values.inputs[0], Some(DynamicValue::Int(35))));
    assert!(values.outputs.is_empty());

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn get_input_output_values_with_none_binding() -> anyhow::Result<()> {
    let mut func_lib = test_func_lib(TestFuncHooks {
        get_a: Arc::new(move || Ok(1)),
        get_b: Arc::new(move || 11),
        print: Arc::new(move |_| {}),
    });

    let mut graph = test_graph();
    let mut execution_graph = ExecutionGraph::default();

    func_lib.by_name_mut("mult").unwrap().inputs[1].required = false;
    let mult = graph.by_name_mut("mult").unwrap();
    mult.inputs[1].binding = Binding::None;
    let mult_id = mult.id;

    execution_graph.update(&graph, &func_lib);
    execution_graph.execute_terminals().await?;

    let values = execution_graph.get_argument_values(&mult_id).unwrap();

    assert_eq!(values.inputs.len(), 2);
    assert!(values.inputs[0].is_some());
    assert!(values.inputs[1].is_none());

    Ok(())
}

#[test]
fn get_input_output_values_before_execution() -> anyhow::Result<()> {
    let graph = test_graph();
    let func_lib = test_func_lib(TestFuncHooks::default());
    let mut execution_graph = ExecutionGraph::default();

    execution_graph.update(&graph, &func_lib);

    let sum_id = graph.by_name("sum").unwrap().id;
    let values = execution_graph.get_argument_values(&sum_id).unwrap();

    assert_eq!(values.inputs.len(), 2);
    assert!(values.inputs[0].is_none());
    assert!(values.inputs[1].is_none());
    assert!(values.outputs.is_empty());

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn node_error_propagates_to_dependents() -> anyhow::Result<()> {
    let graph = test_graph();
    let func_lib = test_func_lib(TestFuncHooks {
        get_a: Arc::new(|| Err(anyhow::anyhow!("Intentional failure in get_a"))),
        get_b: Arc::new(|| 42),
        print: Arc::new(|_| {}),
    });

    let mut execution_graph = ExecutionGraph::default();
    execution_graph.update(&graph, &func_lib);

    let stats = execution_graph.execute_terminals().await?;

    let get_a = execution_graph.by_name("get_a").unwrap();
    assert!(get_a.error.is_some());
    assert!(get_a.output_values.is_none());

    let get_b = execution_graph.by_name("get_b").unwrap();
    assert!(get_b.error.is_none());
    assert!(get_b.output_values.is_some());
    assert!(
        get_b.output_values.as_ref().unwrap()[0]
            .as_f64()
            .unwrap()
            .approximately_eq(42.0)
    );

    let sum = execution_graph.by_name("sum").unwrap();
    assert!(sum.error.is_some());
    assert!(sum.output_values.is_none());
    assert!(
        sum.error
            .as_ref()
            .unwrap()
            .to_string()
            .contains("upstream error")
    );

    let mult = execution_graph.by_name("mult").unwrap();
    assert!(mult.error.is_some());
    assert!(mult.output_values.is_none());
    assert!(
        mult.error
            .as_ref()
            .unwrap()
            .to_string()
            .contains("upstream error")
    );

    let print = execution_graph.by_name("print").unwrap();
    assert!(print.error.is_some());
    assert!(print.output_values.is_none());

    assert_eq!(stats.node_errors.len(), 4);

    let get_a_error = stats
        .node_errors
        .iter()
        .find(|e| e.node_id == get_a.id)
        .unwrap();
    assert!(
        get_a_error
            .error
            .to_string()
            .contains("Intentional failure")
    );

    let sum_error = stats
        .node_errors
        .iter()
        .find(|e| e.node_id == sum.id)
        .unwrap();
    assert!(sum_error.error.to_string().contains("upstream error"));

    Ok(())
}
