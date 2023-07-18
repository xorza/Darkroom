use std::cell::RefCell;
use std::rc::Rc;
use std::str::FromStr;

use crate::compute::Compute;
use crate::data::{DataType, DynamicValue, StaticValue};
use crate::function::{Function, FunctionId, InputInfo, OutputInfo};
use crate::graph::{Binding, FunctionBehavior, Graph};
use crate::lambda_invoker::LambdaInvoker;
use crate::preprocess::Preprocess;
use crate::runtime_graph::{InvokeContext, RuntimeGraph};

struct TestValues {
    a: i64,
    b: i64,
    result: i64,
}


fn create_compute<GetA, GetB, SetResult>(
    get_a: GetA, get_b: GetB, result: SetResult,
) -> anyhow::Result<Compute>
where
    SetResult: Fn(i64) + 'static,
    GetA: Fn() -> i64 + 'static,
    GetB: Fn() -> i64 + 'static,
{
    let mut invoker = LambdaInvoker::default();

    // print
    invoker.add_lambda(
        Function {
            self_id: FunctionId::from_str("f22cd316-1cdf-4a80-b86c-1277acd1408a")?,
            name: "print".to_string(),
            behavior: FunctionBehavior::Active,
            is_output: true,
            inputs: vec![InputInfo {
                name: "message".to_string(),
                is_required: true,
                data_type: DataType::Int,
                default_value: None,
                variants: None,
            }],
            outputs: vec![],
        },
        move |_, inputs, _| {
            result(inputs[0].as_ref().unwrap().as_int());
        });
    // val 1
    invoker.add_lambda(
        Function {
            self_id: FunctionId::from_str("d4d27137-5a14-437a-8bb5-b2f7be0941a2")?,
            name: "get_a".to_string(),
            behavior: FunctionBehavior::Passive,
            is_output: false,
            inputs: vec![],
            outputs: vec![
                OutputInfo {
                    name: "value".to_string(),
                    data_type: DataType::Int,
                }
            ],
        },
        move |_, _, outputs| {
            outputs[0] = DynamicValue::from(get_a() as f64).into();
        });
    // val 2
    invoker.add_lambda(
        Function {
            self_id: FunctionId::from_str("a937baff-822d-48fd-9154-58751539b59b")?,
            name: "get_b".to_string(),
            behavior: FunctionBehavior::Passive,
            is_output: false,
            inputs: vec![],
            outputs: vec![
                OutputInfo {
                    name: "value".to_string(),
                    data_type: DataType::Int,
                }
            ],
        },
        move |_, _, outputs| {
            outputs[0] = DynamicValue::from(get_b()).into();
        });
    // sum
    invoker.add_lambda(
        Function {
            self_id: FunctionId::from_str("2d3b389d-7b58-44d9-b3d1-a595765b21a5")?,
            name: "sum".to_string(),
            behavior: FunctionBehavior::Active,
            is_output: true,
            inputs: vec![
                InputInfo {
                    name: "a".to_string(),
                    is_required: true,
                    data_type: DataType::Int,
                    default_value: None,
                    variants: None,
                },
                InputInfo {
                    name: "b".to_string(),
                    is_required: true,
                    data_type: DataType::Int,
                    default_value: None,
                    variants: None,
                },
            ],
            outputs: vec![
                OutputInfo {
                    name: "result".to_string(),
                    data_type: DataType::Int,
                }
            ],
        },
        |ctx, inputs, outputs| {
            let a: i64 = inputs[0].as_ref().unwrap().as_int();
            let b: i64 = inputs[1].as_ref().unwrap().as_int();
            outputs[0] = DynamicValue::from(a + b).into();
            ctx.set(a + b);
        });
    // mult
    invoker.add_lambda(
        Function {
            self_id: FunctionId::from_str("432b9bf1-f478-476c-a9c9-9a6e190124fc")?,
            name: "mult".to_string(),
            behavior: FunctionBehavior::Active,
            is_output: true,
            inputs: vec![
                InputInfo {
                    name: "a".to_string(),
                    is_required: true,
                    data_type: DataType::Int,
                    default_value: None,
                    variants: None,
                },
                InputInfo {
                    name: "b".to_string(),
                    is_required: true,
                    data_type: DataType::Int,
                    default_value: None,
                    variants: None,
                },
            ],
            outputs: vec![
                OutputInfo {
                    name: "result".to_string(),
                    data_type: DataType::Int,
                }
            ],
        },
        |ctx, inputs, outputs| {
            let a: i64 = inputs[0].as_ref().unwrap().as_int();
            let b: i64 = inputs[1].as_ref().unwrap().as_int();
            outputs[0] = DynamicValue::from(a * b).into();
            ctx.set(a * b);
        });

    Ok(invoker.into())
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

#[test]
fn simple_compute_test() -> anyhow::Result<()> {
    let test_values = Rc::new(RefCell::new(TestValues {
        a: 2,
        b: 5,
        result: 0,
    }));

    let test_values_a = test_values.clone();
    let test_values_b = test_values.clone();
    let test_values_result = test_values.clone();
    let compute = create_compute(
        move || test_values_a.borrow().a,
        move || test_values_b.borrow().b,
        move |result| test_values_result.borrow_mut().result = result,
    )?;

    let mut graph = Graph::from_yaml_file("../test_resources/test_graph.yml")?;
    let preprocess = Preprocess::default();

    let mut runtime_graph = preprocess.run(&graph, &mut RuntimeGraph::default());
    compute.run(&graph, &mut runtime_graph)?;
    assert_eq!(test_values.borrow().result, 35);

    let mut runtime_graph = preprocess.run(&graph, &mut runtime_graph);
    compute.run(&graph, &mut runtime_graph)?;
    assert_eq!(test_values.borrow().result, 35);

    test_values.borrow_mut().b = 7;
    graph.node_by_name_mut("get_b").unwrap().behavior = FunctionBehavior::Active;
    let mut runtime_graph = preprocess.run(&graph, &mut RuntimeGraph::default());
    compute.run(&graph, &mut runtime_graph)?;
    assert_eq!(test_values.borrow().result, 63);

    Ok(())
}

#[test]
fn simple_compute_test_default_input_value() -> anyhow::Result<()> {
    let test_values = Rc::new(RefCell::new(TestValues {
        a: 2,
        b: 5,
        result: 0,
    }));

    let test_values_result = test_values.clone();
    let compute = create_compute(
        || panic!("Unexpected call to get_a"),
        || panic!("Unexpected call to get_b"),
        move |result| test_values_result.borrow_mut().result = result,
    )?;

    let mut graph = Graph::from_yaml_file("../test_resources/test_graph.yml")?;

    {
        let sum_inputs = &mut graph
            .node_by_name_mut("sum").unwrap()
            .inputs;
        sum_inputs[0].const_value = Some(StaticValue::from(29));
        sum_inputs[0].binding = Binding::Const;
        sum_inputs[1].const_value = Some(StaticValue::from(11));
        sum_inputs[1].binding = Binding::Const;
    }

    {
        let mult_inputs = &mut graph
            .node_by_name_mut("mult").unwrap()
            .inputs;
        mult_inputs[1].const_value = Some(StaticValue::from(9));
        mult_inputs[1].binding = Binding::Const;
    }

    let preprocess = Preprocess::default();
    let mut runtime_graph = preprocess.run(&graph, &mut RuntimeGraph::default());

    compute.run(&graph, &mut runtime_graph)?;
    assert_eq!(test_values.borrow().result, 360);

    drop(graph);

    Ok(())
}

#[test]
fn cached_value() -> anyhow::Result<()> {
    let test_values = Rc::new(RefCell::new(TestValues {
        a: 2,
        b: 5,
        result: 0,
    }));

    let test_values_a = test_values.clone();
    let test_values_b = test_values.clone();
    let test_values_result = test_values.clone();
    let compute = create_compute(
        move || {
            let a1 = test_values_a.borrow().a;
            test_values_a.borrow_mut().a += 1;

            a1
        },
        move || {
            let b1 = test_values_b.borrow().b;
            test_values_b.borrow_mut().b += 1;
            if b1 == 6 {
                panic!("Unexpected call to get_b");
            }

            b1
        },
        move |result| test_values_result.borrow_mut().result = result,
    )?;

    let mut graph = Graph::from_yaml_file("../test_resources/test_graph.yml")?;
    graph.node_by_name_mut("sum").unwrap()
        .cache_outputs = false;
    let preprocess = Preprocess::default();

    let mut runtime_graph = preprocess.run(&graph, &mut RuntimeGraph::default());
    compute.run(&graph, &mut runtime_graph)?;

    //assert that both nodes were called
    assert_eq!(test_values.borrow().a, 3);
    assert_eq!(test_values.borrow().b, 6);
    assert_eq!(test_values.borrow().result, 35);

    let mut runtime_graph = preprocess.run(&graph, &mut runtime_graph);
    compute.run(&graph, &mut runtime_graph)?;

    //assert that node a was called again
    assert_eq!(test_values.borrow().a, 4);
    //but node b was cached
    assert_eq!(test_values.borrow().b, 6);
    assert_eq!(test_values.borrow().result, 40);

    drop(graph);

    Ok(())
}

