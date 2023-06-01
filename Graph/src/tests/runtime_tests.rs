use uuid::Uuid;

use crate::graph::*;
use crate::invoke::{Args, Invoker, LambdaInvoker, Value};
use crate::runtime::Runtime;

struct EmptyInvoker {}

impl Invoker for EmptyInvoker {
    fn call(&self, _: &str, _: Uuid, _: &Args, _: &mut Args) {}
}

#[test]
fn simple_run() -> anyhow::Result<()> {
    let graph = Graph::from_yaml_file("./test_resources/test_graph.yml")?;
    let mut runtime = Runtime::new();
    let invoker = EmptyInvoker {};

    let nodes = runtime.run(&graph, &invoker)?;
    assert!(nodes.nodes.iter().all(|_node| _node.executed));
    assert!(nodes.nodes.iter().all(|_node| _node.has_arguments));

    Ok(())
}

#[test]
fn double_run() -> anyhow::Result<()> {
    let graph = Graph::from_yaml_file("./test_resources/test_graph.yml")?;
    let mut runtime = Runtime::new();
    let invoker = EmptyInvoker {};

    runtime.run(&graph, &invoker)?;

    let nodes = runtime.run(&graph, &invoker)?;
    assert!(nodes.nodes.iter().all(|node| node.has_arguments));
    assert!(!nodes.node_by_name("val 1").unwrap().executed);
    assert!(!nodes.node_by_name("val 2").unwrap().executed);
    assert!(!nodes.node_by_name("sum").unwrap().executed);
    assert!(!nodes.node_by_name("mult").unwrap().executed);
    assert!(nodes.node_by_name("print").unwrap().executed);

    Ok(())
}

#[test]
fn node_behavior_active_test()  -> anyhow::Result<()> {
    let mut graph = Graph::from_yaml_file("./test_resources/test_graph.yml")?;
    let mut runtime = Runtime::new();
    let invoker = EmptyInvoker {};

    runtime.run(&graph, &invoker)?;

    graph.node_by_name_mut("val 2").unwrap().behavior = NodeBehavior::Active;
    let nodes = runtime.run(&graph, &invoker)?;
    assert!(nodes.nodes.iter().all(|_node| _node.has_arguments));
    assert!(!nodes.node_by_name("val 1").unwrap().executed);
    assert!(nodes.node_by_name("val 2").unwrap().executed);
    assert!(!nodes.node_by_name("sum").unwrap().executed);
    assert!(nodes.node_by_name("mult").unwrap().executed);
    assert!(nodes.node_by_name("print").unwrap().executed);

    Ok(())
}

#[test]
fn edge_behavior_once_test()  -> anyhow::Result<()>{
    let mut graph = Graph::from_yaml_file("./test_resources/test_graph.yml")?;
    let mut runtime = Runtime::new();
    let invoker = EmptyInvoker {};

    runtime.run(&graph, &invoker)?;

    graph.node_by_name_mut("mult").unwrap()
        .inputs.get_mut(1).unwrap()
        .binding.as_mut().unwrap()
        .behavior = BindingBehavior::Once;

    let nodes = runtime.run(&graph, &invoker)?;
    assert!(nodes.nodes.iter().all(|_node| _node.has_arguments));
    assert!(!nodes.node_by_name("val 1").unwrap().executed);
    assert!(!nodes.node_by_name("val 2").unwrap().executed);
    assert!(!nodes.node_by_name("sum").unwrap().executed);
    assert!(!nodes.node_by_name("mult").unwrap().executed);
    assert!(nodes.node_by_name("print").unwrap().executed);

    Ok(())
}

#[test]
fn edge_behavior_always_test()  -> anyhow::Result<()>{
    let mut graph = Graph::from_yaml_file("./test_resources/test_graph.yml")?;
    let mut runtime = Runtime::new();
    let invoker = EmptyInvoker {};

    runtime.run(&graph, &invoker)?;

    graph.node_by_name_mut("sum").unwrap()
        .inputs.get_mut(0).unwrap()
        .binding.as_mut().unwrap()
        .behavior = BindingBehavior::Always;

    let nodes = runtime.run(&graph, &invoker)?;
    assert!(nodes.nodes.iter().all(|_node| _node.has_arguments));
    assert!(nodes.node_by_name("val 1").unwrap().executed);
    assert!(!nodes.node_by_name("val 2").unwrap().executed);
    assert!(nodes.node_by_name("sum").unwrap().executed);
    assert!(nodes.node_by_name("mult").unwrap().executed);
    assert!(nodes.node_by_name("print").unwrap().executed);

    Ok(())
}

#[test]
fn multiple_runs_with_various_modifications() -> anyhow::Result<()> {
    let mut graph = Graph::from_yaml_file("./test_resources/test_graph.yml")?;
    let mut runtime = Runtime::new();
    let invoker = EmptyInvoker {};

    runtime.run(&graph, &invoker)?;

    {
        let nodes = runtime.run(&graph, &invoker)?;
        assert!(!nodes.node_by_name("val 1").unwrap().executed);
        assert!(!nodes.node_by_name("val 2").unwrap().executed);
        assert!(!nodes.node_by_name("sum").unwrap().executed);
        assert!(!nodes.node_by_name("mult").unwrap().executed);
        assert!(nodes.node_by_name("print").unwrap().executed);
    }
    {
        graph.node_by_name_mut("val 2").unwrap().behavior = NodeBehavior::Active;
        let nodes = runtime.run(&graph, &invoker)?;
        assert!(!nodes.node_by_name("val 1").unwrap().executed);
        assert!(nodes.node_by_name("val 2").unwrap().executed);
        assert!(!nodes.node_by_name("sum").unwrap().executed);
        assert!(nodes.node_by_name("mult").unwrap().executed);
        assert!(nodes.node_by_name("print").unwrap().executed);
    }
    {
        graph.node_by_name_mut("mult").unwrap()
            .inputs.get_mut(1).unwrap()
            .binding.as_mut().unwrap()
            .behavior = BindingBehavior::Once;
        let nodes = runtime.run(&graph, &invoker)?;
        assert!(!nodes.node_by_name("val 1").unwrap().executed);
        assert!(!nodes.node_by_name("val 2").unwrap().executed);
        assert!(!nodes.node_by_name("sum").unwrap().executed);
        assert!(!nodes.node_by_name("mult").unwrap().executed);
        assert!(nodes.node_by_name("print").unwrap().executed);
    }
    {
        graph.node_by_name_mut("sum").unwrap()
            .inputs.get_mut(1).unwrap()
            .binding.as_mut().unwrap()
            .behavior = BindingBehavior::Always;
        let nodes = runtime.run(&graph, &invoker)?;
        assert!(nodes.nodes.iter().all(|_node| _node.has_arguments));
        assert!(nodes.node_by_name("val 1").unwrap().executed);
        assert!(nodes.node_by_name("val 2").unwrap().executed);
        assert!(nodes.node_by_name("sum").unwrap().executed);
        assert!(nodes.node_by_name("mult").unwrap().executed);
        assert!(nodes.node_by_name("print").unwrap().executed);
    }

    Ok(())
}


#[test]
fn simple_compute_test() -> anyhow::Result<()> {
    static mut RESULT: i64 = 0;
    static mut A: i64 = 2;
    static mut B: i64 = 5;

    let mut invoker = LambdaInvoker::new();
    invoker.add_lambda("print", |_, inputs, _| unsafe {
        RESULT = inputs[0].as_int();
    });
    invoker.add_lambda("val 1", |_, _, outputs| {
        outputs[0] = Value::from(unsafe { A });
    });
    invoker.add_lambda("val 2", |_, _, outputs| {
        outputs[0] = Value::from(unsafe { B });
    });
    invoker.add_lambda("sum", |_, inputs, outputs| {
        let a: i64 = inputs[0].as_int();
        let b: i64 = inputs[1].as_int();
        outputs[0] = Value::from(a + b);
    });
    invoker.add_lambda("mult", |_, inputs, outputs| {
        let a: i64 = inputs[0].as_int();
        let b: i64 = inputs[1].as_int();
        outputs[0] = Value::from(a * b);
    });

    let mut graph = Graph::from_yaml_file("./test_resources/test_graph.yml")?;
    let mut compute = Runtime::new();

    compute.run(&graph, &invoker)?;
    assert_eq!(unsafe { RESULT }, 35);

    compute.run(&graph, &invoker)?;
    assert_eq!(unsafe { RESULT }, 35);

    unsafe { B = 7; }
    graph.node_by_name_mut("val 2").unwrap().behavior = NodeBehavior::Active;
    compute.run(&graph, &invoker)?;
    assert_eq!(unsafe { RESULT }, 49);

    graph
        .node_by_name_mut("sum").unwrap()
        .inputs.get_mut(0).unwrap()
        .binding.as_mut().unwrap().behavior = BindingBehavior::Always;

    compute.run(&graph, &invoker)?;
    assert_eq!(unsafe { RESULT }, 63);

    drop(graph);

    Ok(())
}
