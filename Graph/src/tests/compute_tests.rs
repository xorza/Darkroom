use std::str::FromStr;

use crate::compute::{Compute, ComputeCache, InvokeContext, LambdaCompute};
use crate::data::Value;
use crate::functions::FunctionId;
use crate::graph::{Binding, FunctionBehavior, Graph};
use crate::preprocess::Preprocess;

static mut RESULT: i64 = 0;
static mut A: i64 = 2;
static mut B: i64 = 5;

fn setup() {
    unsafe {
        RESULT = 0;
        A = 2;
        B = 5;
    }
}

fn create_compute<GetA, GetB, SetResult>(
    get_a: GetA, get_b: GetB, result: SetResult,
) -> anyhow::Result<LambdaCompute>
where
    SetResult: Fn(i64) + 'static,
    GetA: Fn() -> i64 + 'static,
    GetB: Fn() -> i64 + 'static,
{
    setup();

    let mut compute = LambdaCompute::default();

    // print func
    compute.add_lambda(
        FunctionId::from_str("f22cd316-1cdf-4a80-b86c-1277acd1408a")?,
        move |_, inputs, _| {
            result(inputs[0].as_ref().unwrap().as_int());
        });
    // val 1 func
    compute.add_lambda(
        FunctionId::from_str("d4d27137-5a14-437a-8bb5-b2f7be0941a2")?,
        move |_, _, outputs| {
            outputs[0] = Value::from(get_a()).into();
        });
    // val 2 func
    compute.add_lambda(
        FunctionId::from_str("a937baff-822d-48fd-9154-58751539b59b")?,
        move |_, _, outputs| {
            outputs[0] = Value::from(get_b()).into();
        });
    // sum func
    compute.add_lambda(
        FunctionId::from_str("2d3b389d-7b58-44d9-b3d1-a595765b21a5")?,
        |ctx, inputs, outputs| {
            let a: i64 = inputs[0].as_ref().unwrap().as_int();
            let b: i64 = inputs[1].as_ref().unwrap().as_int();
            outputs[0] = Value::from(a + b).into();
            ctx.set(a + b);
        });
    // mult func
    compute.add_lambda(
        FunctionId::from_str("432b9bf1-f478-476c-a9c9-9a6e190124fc")?,
        |ctx, inputs, outputs| {
            let a: i64 = inputs[0].as_ref().unwrap().as_int();
            let b: i64 = inputs[1].as_ref().unwrap().as_int();
            outputs[0] = Value::from(a * b).into();
            ctx.set(a * b);
        });

    Ok(compute)
}

#[test]
fn simple_compute_test_default_input_value() -> anyhow::Result<()> {
    setup();

    let compute = create_compute(
        || panic!("Unexpected call to get_a"),
        || panic!("Unexpected call to get_b"),
        |result| unsafe { RESULT = result; },
    )?;

    let mut graph = Graph::from_yaml_file("../test_resources/test_graph.yml")?;

    {
        let sum_inputs = &mut graph
            .node_by_name_mut("sum").unwrap()
            .inputs;
        sum_inputs[0].const_value = Some(Value::from(29));
        sum_inputs[0].binding = Binding::Const;
        sum_inputs[1].const_value = Some(Value::from(11));
        sum_inputs[1].binding = Binding::Const;
    }

    {
        let mult_inputs = &mut graph
            .node_by_name_mut("mult").unwrap()
            .inputs;
        mult_inputs[1].const_value = Some(Value::from(9));
        mult_inputs[1].binding = Binding::Const;
    }

    let preprocess = Preprocess::default();
    let preprocess_info = preprocess.run(&graph);
    let mut compute_cache = ComputeCache::default();

    let _compute_info = compute.run(&graph, &preprocess_info, &mut compute_cache)?;
    assert_eq!(unsafe { RESULT }, 360);

    drop(graph);

    Ok(())
}

#[test]
fn simple_compute_test() -> anyhow::Result<()> {
    setup();

    let compute = create_compute(
        || unsafe { A },
        || unsafe { B },
        |result| unsafe { RESULT = result; },
    )?;

    let mut graph = Graph::from_yaml_file("../test_resources/test_graph.yml")?;
    let preprocess = Preprocess::default();
    let mut compute_cache = ComputeCache::default();

    let preprocess_info = preprocess.run(&graph);
    let _compute_info = compute.run(&graph, &preprocess_info, &mut compute_cache)?;
    assert_eq!(unsafe { RESULT }, 35);

    let preprocess_info = preprocess.run(&graph);
    let _compute_info = compute.run(&graph, &preprocess_info, &mut compute_cache)?;
    assert_eq!(unsafe { RESULT }, 35);
    assert_eq!(compute_cache.contexts.len(), 2);

    unsafe { B = 7; }
    graph.node_by_name_mut("val2").unwrap().behavior = FunctionBehavior::Active;
    let preprocess_info = preprocess.run(&graph);
    let _compute_info = compute.run(&graph, &preprocess_info, &mut compute_cache)?;
    assert_eq!(unsafe { RESULT }, 63);

    Ok(())
}

#[test]
fn invoke_context_test() -> anyhow::Result<()> {
    fn box_test_(ctx: &mut InvokeContext) {
        let n = *ctx.get_or_default::<u32>();
        assert_eq!(n, 0);
        let n = ctx.get_or_default::<i32>();
        assert_eq!(*n, 0);
        *n = 13;
    }

    let mut ctx = InvokeContext::default();
    box_test_(&mut ctx);
    let n = ctx.get_or_default::<i32>();
    assert_eq!(*n, 13);
    let n = *ctx.get_or_default::<u32>();
    assert_eq!(n, 0);

    Ok(())
}

