//! Shared test fixtures: a 5-func sample library and a small graph that
//! wires it together. Kept out of production modules so `function.rs` /
//! `graph.rs` don't cross-import from the prelude for fixture code.
//! Darkroom's `session.rs` also uses `test_func_lib` as a demo/sample func
//! set on startup — keep that in mind before renaming.

use std::sync::Arc;

use crate::async_lambda;
use crate::data::DataType;
use crate::graph::{CacheMode, Graph, InputPort, Node, NodeId};
use crate::library::Library;
use crate::node::function::{Func, FuncInput, FuncOutput};

pub struct TestFuncHooks {
    pub get_a: Arc<dyn Fn() -> anyhow::Result<i64> + Send + Sync + 'static>,
    pub get_b: Arc<dyn Fn() -> i64 + Send + Sync + 'static>,
    pub print: Arc<dyn Fn(i64) + Send + Sync + 'static>,
}

impl Default for TestFuncHooks {
    fn default() -> Self {
        Self {
            get_a: Arc::new(|| panic!("Unexpected call to get_a")),
            get_b: Arc::new(|| panic!("Unexpected call to get_b")),
            print: Arc::new(|_| panic!("Unexpected call to print")),
        }
    }
}

pub fn test_func_lib(hooks: TestFuncHooks) -> Library {
    let TestFuncHooks {
        get_a,
        get_b,
        print,
    } = hooks;

    // Every fixture func opts into `Ram` caching (the pre-`None`-default behavior)
    // so the execution tests built on this library exercise cross-run cache reuse;
    // a plain `Func` now defaults to `CacheMode::None`.
    [
        Func::new("432b9bf1-f478-476c-a9c9-9a6e190124fc", "mult")
            .description("Multiplies two integer values (A * B)")
            .category("Debug")
            .default_cache_mode(CacheMode::Ram)
            .pure()
            .input(FuncInput::required("A", DataType::Int))
            .input(FuncInput::optional("B", DataType::Int))
            .output(FuncOutput::new("Prod", DataType::Int))
            .lambda(async_lambda!(move |_, state, _, inputs, _, outputs| {
                assert_eq!(inputs.len(), 2);
                assert_eq!(outputs.len(), 1);

                let a: i64 = inputs[0].value.as_i64().unwrap();
                let b: i64 = inputs[1].value.as_i64().unwrap_or(1);
                outputs[0] = (a * b).into();
                state.set(a * b);

                Ok(())
            })),
        Func::new("d4d27137-5a14-437a-8bb5-b2f7be0941a2", "get_a")
            .description("Returns the value from test hook A")
            .category("Debug")
            .default_cache_mode(CacheMode::Ram)
            .pure()
            .output(FuncOutput::new("Int32 Value", DataType::Int))
            .lambda(async_lambda!(
                move |_, _, _, _, _, outputs| { get_a = Arc::clone(&get_a) } => {
                    assert_eq!(outputs.len(), 1);
                    outputs[0] = (get_a()? as f64).into();
                    Ok(())
                }
            )),
        Func::new("a937baff-822d-48fd-9154-58751539b59b", "get_b")
            .description("Returns the value from test hook B")
            .category("Debug")
            .default_cache_mode(CacheMode::Ram)
            .pure()
            .output(FuncOutput::new("Int32 Value", DataType::Int))
            .lambda(async_lambda!(
                move |_, _, _, _, _, outputs| { get_b = Arc::clone(&get_b) } => {
                    assert_eq!(outputs.len(), 1);
                    outputs[0] = (get_b() as f64).into();
                    Ok(())
                }
            )),
        Func::new("2d3b389d-7b58-44d9-b3d1-a595765b21a5", "sum")
            .description("Adds two integer values (A + B)")
            .category("Debug")
            .default_cache_mode(CacheMode::Ram)
            .pure()
            .input(FuncInput::required("A", DataType::Int))
            .input(FuncInput::optional("B", DataType::Int))
            .output(FuncOutput::new("Sum", DataType::Int))
            .lambda(async_lambda!(move |_, state, _, inputs, _, outputs| {
                assert_eq!(inputs.len(), 2);
                assert_eq!(outputs.len(), 1);
                let a: i64 = inputs[0].value.as_i64().unwrap();
                let b: i64 = inputs[1].value.as_i64().unwrap_or_default();
                state.set(a + b);
                outputs[0] = (a + b).into();
                Ok(())
            })),
        Func::new("f22cd316-1cdf-4a80-b86c-1277acd1408a", "Print")
            .description("Outputs an integer value via the test print hook")
            .category("Debug")
            .default_cache_mode(CacheMode::Ram)
            .sink()
            .input(FuncInput::required("message", DataType::Int))
            .lambda(async_lambda!(
                move |_, _, _, inputs, _, _| { print = Arc::clone(&print) } => {
                    assert_eq!(inputs.len(), 1);
                    print(inputs[0].value.as_i64().unwrap());
                    Ok(())
                }
            )),
    ]
    .into()
}

pub fn test_graph() -> Graph {
    let library = test_func_lib(TestFuncHooks::default());

    let mut graph = Graph::default();

    let mult_node_id: NodeId = "579ae1d6-10a3-4906-8948-135cb7d7508b".into();
    let get_a_node_id: NodeId = "5f110618-8faa-4629-8f5d-473c236de7d1".into();
    let get_b_node_id: NodeId = "6fc6b533-c375-451c-ba3a-a14ea217cb30".into();
    let sum_node_id: NodeId = "999c4d37-e0eb-4856-be3f-ad2090c84d8c".into();
    let print_node_id: NodeId = "b88ab7e2-17b7-46cb-bc8e-b428bb45141e".into();

    let get_a_func = library.by_name("get_a").unwrap();
    let get_b_func = library.by_name("get_b").unwrap();
    let sum_func = library.by_name("sum").unwrap();
    let mult_func = library.by_name("mult").unwrap();
    let print_func = library.by_name("Print").unwrap();

    let mut get_a_node: Node = get_a_func.into();
    get_a_node.id = get_a_node_id;
    graph.add(get_a_node);

    let mut get_b_node: Node = get_b_func.into();
    get_b_node.id = get_b_node_id;
    graph.add(get_b_node);

    let mut sum_node: Node = sum_func.into();
    sum_node.id = sum_node_id;
    graph.add(sum_node);

    let mut mult_node: Node = mult_func.into();
    mult_node.id = mult_node_id;
    graph.add(mult_node);

    let mut print_node: Node = print_func.into();
    print_node.id = print_node_id;
    graph.add(print_node);

    graph.set_input_binding(InputPort::new(sum_node_id, 0), (get_a_node_id, 0).into());
    graph.set_input_binding(InputPort::new(sum_node_id, 1), (get_b_node_id, 0).into());
    graph.set_input_binding(InputPort::new(mult_node_id, 0), (sum_node_id, 0).into());
    graph.set_input_binding(InputPort::new(mult_node_id, 1), (get_b_node_id, 0).into());
    graph.set_input_binding(InputPort::new(print_node_id, 0), (mult_node_id, 0).into());

    graph.validate();

    graph
}
