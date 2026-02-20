use std::sync::Arc;

use super::*;
use crate::data::{DynamicValue, StaticValue};
use crate::function::{TestFuncHooks, test_func_lib};
use crate::graph::{NodeBehavior, test_graph};
use common::{FloatExt, SerdeFormat};
use tokio::sync::Mutex;

// === Shared Helpers ===

fn execution_node_names_in_order(execution_graph: &ExecutionGraph) -> Vec<String> {
    execution_graph
        .e_node_execute_order
        .iter()
        .map(|&e_node_idx| execution_graph.e_nodes[e_node_idx].name.clone())
        .collect()
}

fn default_hooks() -> TestFuncHooks {
    TestFuncHooks {
        get_a: Arc::new(move || Ok(1)),
        get_b: Arc::new(move || 11),
        print: Arc::new(move |_| {}),
    }
}

// === Graph Structure ===

mod graph_structure {
    use super::*;

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

        // usage_count: get_a→sum[0], get_b→sum[1]+mult[1], sum→mult[0], mult→print[0]
        assert_eq!(get_a.outputs[0].usage_count, 1);
        assert_eq!(get_b.outputs[0].usage_count, 2);
        assert_eq!(sum.outputs[0].usage_count, 1);
        assert_eq!(mult.outputs[0].usage_count, 1);

        // Leaf nodes have no input dependencies so inputs_updated=false
        assert!(!get_a.inputs_updated);
        assert!(!get_b.inputs_updated);
        assert!(sum.inputs_updated);
        assert!(mult.inputs_updated);
        assert!(print.inputs_updated);

        assert!(print.terminal);

        Ok(())
    }

    #[test]
    fn updates_after_graph_change() -> anyhow::Result<()> {
        let mut graph = test_graph();
        let func_lib = test_func_lib(TestFuncHooks::default());
        let mut execution_graph = ExecutionGraph::default();

        execution_graph.update(&graph, &func_lib);

        // Rewire mult to get_a and get_b directly (bypassing sum)
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
        // Now each source has exactly 1 consumer (sum is no longer in the path)
        assert_eq!(get_a.outputs[0].usage_count, 1);
        assert_eq!(get_b.outputs[0].usage_count, 1);
        assert_eq!(mult.outputs[0].usage_count, 1);

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
}

// === Missing Inputs ===

mod missing_inputs {
    use super::*;

    #[test]
    fn required_missing_propagates_downstream() -> anyhow::Result<()> {
        let mut graph = test_graph();
        let func_lib = test_func_lib(TestFuncHooks::default());

        // Remove sum's first input binding (required by default)
        graph.by_name_mut("sum").unwrap().inputs[0].binding = Binding::None;

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib);
        execution_graph.prepare_execution(true, false, &[])?;

        let get_b = execution_graph.by_name("get_b").unwrap();
        let sum = execution_graph.by_name("sum").unwrap();
        let mult = execution_graph.by_name("mult").unwrap();
        let print = execution_graph.by_name("print").unwrap();

        // get_b has no missing inputs (no inputs at all)
        assert!(!get_b.missing_required_inputs);
        // sum is missing input[0], propagates to downstream mult and print
        assert!(sum.missing_required_inputs);
        assert!(mult.missing_required_inputs);
        assert!(print.missing_required_inputs);

        // Nothing should be scheduled for execution
        assert_eq!(execution_graph.e_node_execute_order.len(), 0);

        Ok(())
    }

    #[test]
    fn non_required_missing_does_not_propagate() -> anyhow::Result<()> {
        let mut graph = test_graph();
        let mut func_lib = test_func_lib(TestFuncHooks::default());
        let mut execution_graph = ExecutionGraph::default();

        // Remove sum's first input, but make mult's first input optional
        graph.by_name_mut("sum").unwrap().inputs[0].binding = Binding::None;
        func_lib.by_name_mut("mult").unwrap().inputs[0].required = false;

        execution_graph.update(&graph, &func_lib);
        execution_graph.prepare_execution(true, false, &[])?;

        let sum = execution_graph.by_name("sum").unwrap();
        let mult = execution_graph.by_name("mult").unwrap();
        let print = execution_graph.by_name("print").unwrap();

        // sum still missing, but mult/print are fine because mult[0] is optional
        assert!(sum.missing_required_inputs);
        assert!(!mult.missing_required_inputs);
        assert!(!print.missing_required_inputs);

        // Only get_b→mult→print should execute (sum excluded)
        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["get_b", "mult", "print"]
        );

        Ok(())
    }
}

// === Const Bindings ===

mod const_bindings {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn const_binding_tracks_changes() -> anyhow::Result<()> {
        let mut graph = test_graph();
        let func_lib = test_func_lib(default_hooks());
        let mut execution_graph = ExecutionGraph::default();

        let mult = graph.by_name_mut("mult").unwrap();
        mult.inputs[0].binding = Binding::Const(StaticValue::Int(3));
        mult.inputs[1].binding = Binding::Const(StaticValue::Int(5));

        execution_graph.update(&graph, &func_lib);

        let mult = execution_graph.by_name("mult").unwrap();
        assert!(mult.inputs[0].binding_changed);
        assert!(mult.inputs[1].binding_changed);

        execution_graph.execute_terminals().await?;

        // Only mult and print execute (upstream nodes excluded by const)
        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["mult", "print"]
        );

        let mult = execution_graph.by_name("mult").unwrap();
        assert!(mult.inputs_updated);
        // After execution, binding_changed is cleared
        assert!(!mult.inputs[0].binding_changed);
        assert!(!mult.inputs[0].dependency_wants_execute);
        assert!(!mult.inputs[1].binding_changed);
        assert!(!mult.inputs[1].dependency_wants_execute);

        // Re-run with same bindings: mult is cached, only print re-executes
        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;

        assert_eq!(execution_node_names_in_order(&execution_graph), ["print"]);

        let mult = execution_graph.by_name("mult").unwrap();
        assert!(!mult.inputs_updated);
        assert!(!mult.inputs[0].binding_changed);
        assert!(!mult.inputs[1].binding_changed);

        // Change one const: mult re-executes
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

        // Same const value: no re-execution of mult
        graph.by_name_mut("mult").unwrap().inputs[0].binding = Binding::Const(StaticValue::Int(3));
        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;

        assert_eq!(execution_node_names_in_order(&execution_graph), ["print"]);

        // Different const value: mult re-executes
        let mult = graph.by_name_mut("mult").unwrap();
        mult.inputs[0].binding = Binding::Const(StaticValue::Int(4));
        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["mult", "print"]
        );

        // Stable again
        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;

        assert_eq!(execution_node_names_in_order(&execution_graph), ["print"]);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn const_excludes_upstream_node() -> anyhow::Result<()> {
        let func_lib = test_func_lib(default_hooks());

        let mut graph = test_graph();
        let mut execution_graph = ExecutionGraph::default();

        // Replace sum[0] (get_a) with a const — get_a is no longer needed
        let sum = graph.by_name_mut("sum").unwrap();
        sum.inputs[0].binding = Binding::Const(33.into());

        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["get_b", "sum", "mult", "print"]
        );

        // Also unbind sum[1] — now sum has all const/none inputs, no upstream needed
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
        let func_lib = test_func_lib(default_hooks());

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

        // Switch from const back to bind — sum must re-execute
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
    async fn optional_input_binding_change_recomputes() -> anyhow::Result<()> {
        let func_lib = test_func_lib(default_hooks());

        let mut graph = test_graph();
        let mut execution_graph = ExecutionGraph::default();

        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;

        // Switch mult inputs to const/none
        let mult = graph.by_name_mut("mult").unwrap();
        mult.inputs[0].binding = Binding::Const(2.into());
        mult.inputs[1].binding = Binding::None;

        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["mult", "print"]
        );

        // Stable on rerun
        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;

        assert_eq!(execution_node_names_in_order(&execution_graph), ["print"]);

        Ok(())
    }
}

// === Behavior (Pure / Impure / Once) ===

mod behavior {
    use super::*;

    #[test]
    fn pure_node_skips_on_rerun() -> anyhow::Result<()> {
        let mut graph = test_graph();
        let mut func_lib = test_func_lib(TestFuncHooks::default());

        graph.by_name_mut("get_b").unwrap().behavior = NodeBehavior::AsFunction;
        func_lib.by_name_mut("get_b").unwrap().behavior = FuncBehavior::Pure;

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib);
        execution_graph.prepare_execution(true, false, &[])?;

        assert!(execution_node_names_in_order(&execution_graph).contains(&"get_b".to_string()));

        // Simulate cached output — pure node should skip
        execution_graph.by_name_mut("get_b").unwrap().output_values =
            Some(vec![DynamicValue::Int(7)]);

        execution_graph.update(&graph, &func_lib);
        execution_graph.prepare_execution(true, false, &[])?;

        assert!(!execution_node_names_in_order(&execution_graph).contains(&"get_b".to_string()));

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn default_node_skips_on_rerun() -> anyhow::Result<()> {
        let graph = test_graph();
        let func_lib = test_func_lib(default_hooks());
        let mut execution_graph = ExecutionGraph::default();

        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph)[2..],
            ["sum", "mult", "print"]
        );

        // Second run: only print (impure terminal) re-executes, others cached
        let exe_stats = execution_graph.execute_terminals().await?;
        assert_eq!(execution_node_names_in_order(&execution_graph), ["print"]);
        assert_eq!(exe_stats.cached_nodes.len(), 4);

        Ok(())
    }

    #[test]
    fn impure_node_always_invoked() -> anyhow::Result<()> {
        let mut graph = test_graph();
        let mut func_lib = test_func_lib(TestFuncHooks::default());

        graph.by_name_mut("get_b").unwrap().behavior = NodeBehavior::AsFunction;
        func_lib.by_name_mut("get_b").unwrap().behavior = FuncBehavior::Impure;

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib);

        // Even with cached output, impure node still wants to execute
        execution_graph.by_name_mut("get_b").unwrap().output_values =
            Some(vec![DynamicValue::Int(7)]);
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

        // With cached output, Once node is skipped even though func is Impure
        execution_graph.by_name_mut("get_b").unwrap().output_values =
            Some(vec![DynamicValue::Int(7)]);
        execution_graph.update(&graph, &func_lib);
        execution_graph.prepare_execution(true, false, &[])?;

        // get_b excluded, but rest still runs
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

        // Second run: Once node cached
        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;

        assert_eq!(execution_node_names_in_order(&execution_graph), ["print"]);

        // Change binding: Once node must recompute
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

        // Switch to const bindings
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

        // Switch back to bind (swapped) — still recomputes
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

    #[tokio::test(flavor = "multi_thread")]
    async fn once_node_toggle_refreshes_upstream() -> anyhow::Result<()> {
        let func_lib = test_func_lib(default_hooks());

        let mut graph = test_graph();
        let mut execution_graph = ExecutionGraph::default();

        let sum = graph.by_name_mut("sum").unwrap();
        sum.inputs[0].binding = Binding::Const(2.into());
        sum.inputs[1].binding = Binding::Const(21.into());

        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;

        // Change sum's const and set mult to Once simultaneously
        let sum = graph.by_name_mut("sum").unwrap();
        sum.inputs[0].binding = Binding::Const(12.into());
        let mult = graph.by_name_mut("mult").unwrap();
        mult.behavior = NodeBehavior::Once;

        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;

        let sum = execution_graph.by_name("sum").unwrap();
        assert!(!sum.cached);

        // Once node was just set, it has cached output, so only print runs
        assert_eq!(execution_node_names_in_order(&execution_graph), ["print"]);

        // Toggle mult back to AsFunction — sum now needs to re-execute
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
}

// === Cycle Detection ===

mod cycle_detection {
    use super::*;

    #[test]
    fn returns_error_with_node_id() {
        let mut graph = test_graph();
        let func_lib = test_func_lib(TestFuncHooks::default());

        // Create cycle: sum[0] ← mult (mult already depends on sum)
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
            _ => panic!("Unexpected error: {err:?}"),
        }
    }
}

// === Invalidation & State Reset ===

mod invalidation {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn invalidate_clears_dependents() -> anyhow::Result<()> {
        let graph = test_graph();
        let func_lib = test_func_lib(default_hooks());

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;

        // All nodes should have output values after execution
        assert!(
            execution_graph
                .by_name("sum")
                .unwrap()
                .output_values
                .is_some()
        );
        assert!(
            execution_graph
                .by_name("mult")
                .unwrap()
                .output_values
                .is_some()
        );

        let sum_id = graph.by_name("sum").unwrap().id;
        execution_graph.invalidate_recursively(vec![sum_id]);

        // sum and its downstream (mult, print) should be invalidated
        assert!(
            execution_graph
                .by_name("sum")
                .unwrap()
                .output_values
                .is_none()
        );
        assert!(
            execution_graph
                .by_name("mult")
                .unwrap()
                .output_values
                .is_none()
        );
        assert!(
            execution_graph
                .by_name("print")
                .unwrap()
                .output_values
                .is_none()
        );

        // Upstream nodes (get_a, get_b) should NOT be invalidated
        assert!(
            execution_graph
                .by_name("get_a")
                .unwrap()
                .output_values
                .is_some()
        );
        assert!(
            execution_graph
                .by_name("get_b")
                .unwrap()
                .output_values
                .is_some()
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn clear_resets_graph() -> anyhow::Result<()> {
        let graph = test_graph();
        let func_lib = test_func_lib(default_hooks());

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;

        assert!(!execution_graph.e_nodes.is_empty());

        execution_graph.clear();

        assert!(execution_graph.e_nodes.is_empty());
        assert!(execution_graph.e_node_process_order.is_empty());
        assert!(execution_graph.e_node_execute_order.is_empty());
        assert!(execution_graph.e_node_terminal_idx.is_empty());

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn reset_states_clears_outputs() -> anyhow::Result<()> {
        let graph = test_graph();
        let func_lib = test_func_lib(default_hooks());

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;

        // Verify outputs exist before reset
        assert!(
            execution_graph
                .by_name("sum")
                .unwrap()
                .output_values
                .is_some()
        );

        execution_graph.reset_states();

        // All output_values and state should be cleared
        for e_node in execution_graph.e_nodes.iter() {
            assert!(
                e_node.output_values.is_none(),
                "node {} should have no output_values",
                e_node.name
            );
            assert!(
                e_node.state.is_none(),
                "node {} should have no state",
                e_node.name
            );
        }

        Ok(())
    }
}

// === Full Execution ===

mod execution {
    use super::*;

    #[derive(Debug)]
    struct TestValues {
        a: i64,
        b: i64,
        result: i64,
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn simple_compute() -> anyhow::Result<()> {
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
        // sum = get_a + get_b = 2 + 5 = 7, mult = sum * get_b = 7 * 5 = 35
        assert_eq!(test_values.try_lock()?.result, 35);

        // Changing external state doesn't recompute (get_b is Once by default)
        test_values.try_lock()?.b = 7;

        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;
        assert_eq!(test_values.try_lock()?.result, 35);

        // Make get_b Impure: now it re-reads the value
        func_lib.by_name_mut("get_b").unwrap().behavior = FuncBehavior::Impure;

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;
        // sum = 2 + 7 = 9, mult = 9 * 7 = 63
        assert_eq!(test_values.try_lock()?.result, 63);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn required_none_binding_is_stable() -> anyhow::Result<()> {
        let func_lib = test_func_lib(default_hooks());

        let mut graph = test_graph();
        let mut execution_graph = ExecutionGraph::default();

        // Make sum's first input None (required) — sum and downstream shouldn't execute
        let sum = graph.by_name_mut("sum").unwrap();
        sum.inputs[0].binding = Binding::None;

        execution_graph.update(&graph, &func_lib);

        execution_graph.execute_terminals().await?;
        let order1 = execution_node_names_in_order(&execution_graph);

        execution_graph.execute_terminals().await?;
        let order2 = execution_node_names_in_order(&execution_graph);

        // Execution order should be stable across runs
        assert_eq!(order1, order2);

        // sum should be marked as missing required inputs
        let sum = execution_graph.by_name("sum").unwrap();
        assert!(sum.missing_required_inputs);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cached_upstream_output_reused_after_rebinding() -> anyhow::Result<()> {
        let func_lib = test_func_lib(default_hooks());

        let mut graph = test_graph();
        let mut execution_graph = ExecutionGraph::default();

        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;

        // Switch mult to const inputs
        let mult = graph.by_name_mut("mult").unwrap();
        mult.inputs[0].binding = Binding::Const(2.into());
        mult.inputs[1].binding = Binding::Const(21.into());

        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["mult", "print"]
        );

        // Switch back to bind from cached get_b — mult re-executes with cached upstream
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
}

// === Argument Values ===

mod argument_values {
    use super::*;

    #[test]
    fn nonexistent_node_returns_none() {
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
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn with_const_bindings() -> anyhow::Result<()> {
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

        // 3 * 5 = 15
        assert_eq!(values.outputs.len(), 1);
        assert!(matches!(values.outputs[0], DynamicValue::Int(15)));

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn with_bound_outputs() -> anyhow::Result<()> {
        let func_lib = test_func_lib(TestFuncHooks {
            get_a: Arc::new(move || Ok(2)),
            get_b: Arc::new(move || 5),
            print: Arc::new(move |_| {}),
        });

        let graph = test_graph();
        let mut execution_graph = ExecutionGraph::default();

        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;

        // sum: inputs are get_a(2.0) and get_b(5.0), output is 2+5=7
        let sum_id = graph.by_name("sum").unwrap().id;
        let values = execution_graph.get_argument_values(&sum_id).unwrap();

        assert_eq!(values.inputs.len(), 2);
        assert!(
            matches!(values.inputs[0], Some(DynamicValue::Float(v)) if v.approximately_eq(2.0))
        );
        assert!(
            matches!(values.inputs[1], Some(DynamicValue::Float(v)) if v.approximately_eq(5.0))
        );
        assert_eq!(values.outputs.len(), 1);
        assert!(matches!(values.outputs[0], DynamicValue::Int(7)));

        // mult: inputs are sum(7) and get_b(5.0), output is 7*5=35
        let mult_id = graph.by_name("mult").unwrap().id;
        let values = execution_graph.get_argument_values(&mult_id).unwrap();

        assert_eq!(values.inputs.len(), 2);
        assert!(matches!(values.inputs[0], Some(DynamicValue::Int(7))));
        assert!(
            matches!(values.inputs[1], Some(DynamicValue::Float(v)) if v.approximately_eq(5.0))
        );
        assert_eq!(values.outputs.len(), 1);
        assert!(matches!(values.outputs[0], DynamicValue::Int(35)));

        // print: input is mult(35), no outputs
        let print_id = graph.by_name("print").unwrap().id;
        let values = execution_graph.get_argument_values(&print_id).unwrap();

        assert_eq!(values.inputs.len(), 1);
        assert!(matches!(values.inputs[0], Some(DynamicValue::Int(35))));
        assert!(values.outputs.is_empty());

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn with_none_binding() -> anyhow::Result<()> {
        let mut func_lib = test_func_lib(default_hooks());

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
        // None binding returns None value
        assert!(values.inputs[1].is_none());

        Ok(())
    }

    #[test]
    fn before_execution() -> anyhow::Result<()> {
        let graph = test_graph();
        let func_lib = test_func_lib(TestFuncHooks::default());
        let mut execution_graph = ExecutionGraph::default();

        execution_graph.update(&graph, &func_lib);

        let sum_id = graph.by_name("sum").unwrap().id;
        let values = execution_graph.get_argument_values(&sum_id).unwrap();

        // Before execution: all inputs are None (no upstream values yet)
        assert_eq!(values.inputs.len(), 2);
        assert!(values.inputs[0].is_none());
        assert!(values.inputs[1].is_none());
        assert!(values.outputs.is_empty());

        Ok(())
    }
}

// === Error Propagation ===

mod error_propagation {
    use super::*;

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

        // get_a fails with error, no outputs
        let get_a = execution_graph.by_name("get_a").unwrap();
        assert!(get_a.error.is_some());
        assert!(get_a.output_values.is_none());

        // get_b succeeds
        let get_b = execution_graph.by_name("get_b").unwrap();
        assert!(get_b.error.is_none());
        assert!(get_b.output_values.is_some());
        assert!(
            get_b.output_values.as_ref().unwrap()[0]
                .as_f64()
                .unwrap()
                .approximately_eq(42.0)
        );

        // sum depends on get_a, gets upstream error
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

        // mult depends on sum, also gets upstream error
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

        // print depends on mult, also gets upstream error
        let print = execution_graph.by_name("print").unwrap();
        assert!(print.error.is_some());
        assert!(print.output_values.is_none());

        // Stats: 4 errors (get_a original + 3 upstream propagated)
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
}

// === Execution Stats ===

mod stats {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn missing_inputs_reported() -> anyhow::Result<()> {
        let mut graph = test_graph();
        let func_lib = test_func_lib(default_hooks());

        // Remove sum's first input (required)
        graph.by_name_mut("sum").unwrap().inputs[0].binding = Binding::None;

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib);
        let stats = execution_graph.execute_terminals().await?;

        // sum[0] should appear in missing_inputs
        let sum_id = graph.by_name("sum").unwrap().id;
        assert!(
            stats
                .missing_inputs
                .iter()
                .any(|p| p.target_id == sum_id && p.port_idx == 0),
            "Expected sum input 0 in missing_inputs, got: {:?}",
            stats.missing_inputs
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn executed_nodes_reported() -> anyhow::Result<()> {
        let graph = test_graph();
        let func_lib = test_func_lib(default_hooks());

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib);
        let stats = execution_graph.execute_terminals().await?;

        // All 5 nodes should be reported as executed
        assert_eq!(stats.executed_nodes.len(), 5);

        // Each node should have a non-negative elapsed time
        for node_stats in &stats.executed_nodes {
            assert!(
                node_stats.elapsed_secs >= 0.0,
                "node {:?} has negative elapsed_secs",
                node_stats.node_id
            );
        }

        // Verify specific node IDs are present
        let sum_id = graph.by_name("sum").unwrap().id;
        let print_id = graph.by_name("print").unwrap().id;
        assert!(stats.executed_nodes.iter().any(|n| n.node_id == sum_id));
        assert!(stats.executed_nodes.iter().any(|n| n.node_id == print_id));

        // No errors on first clean run
        assert!(stats.node_errors.is_empty());
        assert!(stats.missing_inputs.is_empty());

        Ok(())
    }
}
