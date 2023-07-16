use std::str::FromStr;

use crate::compute::Compute;
use crate::data::{DataType, Value};
use crate::functions::{Function, FunctionId, InputInfo, OutputInfo};
use crate::graph::{Binding, FunctionBehavior, Graph};
use crate::lambda_invoker::LambdaInvoker;
use crate::preprocess::Preprocess;
use crate::runtime_graph::{InvokeContext, RuntimeGraph};

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
) -> anyhow::Result<Compute>
where
    SetResult: Fn(i64) + 'static,
    GetA: Fn() -> i64 + 'static,
    GetB: Fn() -> i64 + 'static,
{
    setup();

    let mut invoker = LambdaInvoker::default();

    // print func
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
    invoker.add_lambda(
        Function {
            self_id: FunctionId::from_str("d4d27137-5a14-437a-8bb5-b2f7be0941a2")?,
            name: "val 1".to_string(),
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
            outputs[0] = Value::from(get_a()).into();
        });
    invoker.add_lambda(
        Function {
            self_id: FunctionId::from_str("a937baff-822d-48fd-9154-58751539b59b")?,
            name: "val 2".to_string(),
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
            outputs[0] = Value::from(get_b()).into();
        });
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
            outputs[0] = Value::from(a + b).into();
            ctx.set(a + b);
        });
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
            outputs[0] = Value::from(a * b).into();
            ctx.set(a * b);
        });

    Ok(invoker.into())
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
    let mut runtime_graph = preprocess.run(&graph, &mut RuntimeGraph::default());

    compute.run(&graph, &mut runtime_graph)?;
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

    let mut runtime_graph = preprocess.run(&graph, &mut RuntimeGraph::default());
    compute.run(&graph, &mut runtime_graph)?;
    assert_eq!(unsafe { RESULT }, 35);

    let mut runtime_graph = preprocess.run(&graph, &mut runtime_graph);
    compute.run(&graph, &mut runtime_graph)?;
    assert_eq!(unsafe { RESULT }, 35);

    unsafe { B = 7; }
    graph.node_by_name_mut("val2").unwrap().behavior = FunctionBehavior::Active;
    let mut runtime_graph = preprocess.run(&graph, &mut RuntimeGraph::default());
    compute.run(&graph, &mut runtime_graph)?;
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

