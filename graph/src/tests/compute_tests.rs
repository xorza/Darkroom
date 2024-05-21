use std::sync::Arc;

use parking_lot::Mutex;

use crate::compute::Compute;
use crate::data::StaticValue;
use crate::function::FuncLib;
use crate::graph::{Binding, FuncBehavior, Graph};
use crate::invoke_context::{InvokeCache, Invoker, LambdaInvoker};
use crate::runtime_graph::RuntimeGraph;

struct TestValues {
    a: i64,
    b: i64,
    result: i64,
}

fn create_invoker<GetA, GetB, SetResult>(
    get_a: GetA,
    get_b: GetB,
    result: SetResult,
) -> anyhow::Result<LambdaInvoker>
    where
        SetResult: Fn(i64) + Send + Sync + 'static,
        GetA: Fn() -> i64 + Send + Sync + 'static,
        GetB: Fn() -> i64 + Send + Sync + 'static,
{
    let func_lib = FuncLib::from_yaml_file("../test_resources/test_funcs.yml")?;

    let mut invoker = LambdaInvoker::default();

    // print
    invoker.add_lambda(
        func_lib.func_by_name("print").unwrap().clone(),
        move |_, inputs, _| {
            result(inputs[0].as_int());
        },
    );
    // val 1
    invoker.add_lambda(
        func_lib.func_by_name("get_a").unwrap().clone(),
        move |_, _, outputs| {
            outputs[0] = (get_a() as f64).into();
        },
    );
    // val 2
    invoker.add_lambda(
        func_lib.func_by_name("get_b").unwrap().clone(),
        move |_, _, outputs| {
            outputs[0] = (get_b() as f64).into();
        },
    );
    // sum
    invoker.add_lambda(
        func_lib.func_by_name("sum").unwrap().clone(),
        |ctx, inputs, outputs| {
            let a: i64 = inputs[0].as_int();
            let b: i64 = inputs[1].as_int();
            ctx.set(a + b);
            outputs[0] = (a + b).into();
        },
    );
    // mult
    invoker.add_lambda(
        func_lib.func_by_name("mult").unwrap().clone(),
        |ctx, inputs, outputs| {
            let a: i64 = inputs[0].as_int();
            let b: i64 = inputs[1].as_int();
            outputs[0] = (a * b).into();
            ctx.set(a * b);
        },
    );

    Ok(invoker)
}

#[test]
fn invoke_context_test() -> anyhow::Result<()> {
    fn box_test_(cache: &mut InvokeCache) {
        let n = *cache.get_or_default::<u32>();
        assert_eq!(n, 0);
        let n = cache.get_or_default::<i32>();
        assert_eq!(*n, 0);
        *n = 13;
    }

    let mut cache = InvokeCache::default();
    box_test_(&mut cache);
    let n = cache.get_or_default::<i32>();
    assert_eq!(*n, 13);
    let n = *cache.get_or_default::<u32>();
    assert_eq!(n, 0);

    Ok(())
}

#[test]
fn simple_compute_test() -> anyhow::Result<()> {
    let test_values = Arc::new(Mutex::new(TestValues {
        a: 2,
        b: 5,
        result: 0,
    }));

    let test_values_a = test_values.clone();
    let test_values_b = test_values.clone();
    let test_values_result = test_values.clone();
    let mut invoker = create_invoker(
        move || test_values_a.lock().a,
        move || test_values_b.lock().b,
        move |result| test_values_result.lock().result = result,
    )?;


    let graph = Graph::from_yaml_file("../test_resources/test_graph.yml")?;

    let mut runtime_graph = RuntimeGraph::new(&graph, &invoker.func_lib);
    Compute::default().run(&graph, &invoker.func_lib, &invoker, &mut runtime_graph)?;
    assert_eq!(test_values.lock().result, 35);

    Compute::default().run(&graph, &invoker.func_lib, &invoker, &mut runtime_graph)?;
    assert_eq!(test_values.lock().result, 35);


    test_values.lock().b = 7;
    invoker.func_lib.func_by_name_mut("get_b").unwrap().behavior = FuncBehavior::Active;
    let mut runtime_graph = RuntimeGraph::new(&graph, &invoker.func_lib);
    Compute::default().run(&graph, &invoker.func_lib, &invoker, &mut runtime_graph)?;
    assert_eq!(test_values.lock().result, 63);

    Ok(())
}

#[test]
fn default_input_value() -> anyhow::Result<()> {
    let test_values = Arc::new(Mutex::new(TestValues {
        a: 2,
        b: 5,
        result: 0,
    }));
    let test_values_result = test_values.clone();

    let invoker = create_invoker(
        || panic!("Unexpected call to get_a"),
        || panic!("Unexpected call to get_b"),
        move |result| test_values_result.lock().result = result,
    )?;
    let func_lib = invoker.get_func_lib();
    let compute = Compute::default();

    let mut graph = Graph::from_yaml_file("../test_resources/test_graph.yml")?;

    {
        let sum_inputs = &mut graph.node_by_name_mut("sum").unwrap().inputs;
        sum_inputs[0].const_value = Some(StaticValue::from(29));
        sum_inputs[0].binding = Binding::Const;
        sum_inputs[1].const_value = Some(StaticValue::from(11));
        sum_inputs[1].binding = Binding::Const;
    }

    {
        let mult_inputs = &mut graph.node_by_name_mut("mult").unwrap().inputs;
        mult_inputs[1].const_value = Some(StaticValue::from(9));
        mult_inputs[1].binding = Binding::Const;
    }

    let mut runtime_graph = RuntimeGraph::new(&graph, &func_lib);

    compute.run(&graph, &func_lib, &invoker, &mut runtime_graph)?;
    assert_eq!(test_values.lock().result, 360);

    drop(graph);

    Ok(())
}

#[test]
fn cached_value() -> anyhow::Result<()> {
    let test_values = Arc::new(Mutex::new(TestValues {
        a: 2,
        b: 5,
        result: 0,
    }));

    let test_values_a = test_values.clone();
    let test_values_b = test_values.clone();
    let test_values_result = test_values.clone();
    let mut invoker = create_invoker(
        move || {
            let a1 = test_values_a.lock().a;
            test_values_a.lock().a += 1;

            a1
        },
        move || {
            let b1 = test_values_b.lock().b;
            test_values_b.lock().b += 1;
            if b1 == 6 {
                panic!("Unexpected call to get_b");
            }

            b1
        },
        move |result| test_values_result.lock().result = result,
    )?;

    invoker.func_lib.func_by_name_mut("get_a").unwrap().behavior = FuncBehavior::Active;

    let mut graph = Graph::from_yaml_file("../test_resources/test_graph.yml")?;
    graph.node_by_name_mut("sum").unwrap().cache_outputs = false;

    let mut runtime_graph = RuntimeGraph::new(&graph, &invoker.func_lib);
    Compute::default().run(&graph, &invoker.func_lib, &invoker, &mut runtime_graph)?;

    //assert that both nodes were called
    assert_eq!(test_values.lock().a, 3);
    assert_eq!(test_values.lock().b, 6);
    assert_eq!(test_values.lock().result, 35);

    Compute::default().run(&graph, &invoker.func_lib, &invoker, &mut runtime_graph)?;

    //assert that node a was called again
    assert_eq!(test_values.lock().a, 4);
    //but node b was cached
    assert_eq!(test_values.lock().b, 6);
    assert_eq!(test_values.lock().result, 40);

    drop(graph);

    Ok(())
}
