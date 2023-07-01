use std::str::FromStr;

use uuid::Uuid;

use crate::data::Value;
use crate::graph::*;
use crate::invoke::{Args, Invoker, LambdaInvoker};
use crate::runtime::Runtime;

struct EmptyInvoker {}

impl Invoker for EmptyInvoker {
    fn call(&self, _: Uuid, _: Uuid, _: &Args, _: &mut Args) -> anyhow::Result<()> {
        Ok(())
    }
}


#[test]
fn simple_run() -> anyhow::Result<()> {
    let graph = Graph::from_yaml_file("../test_resources/test_graph.yml")?;
    let mut runtime = Runtime::default();
    let invoker = EmptyInvoker {};

    let nodes = runtime.run(&graph, &invoker)?;
    assert!(nodes.nodes.iter().all(|_node| _node.executed));
    assert!(nodes.nodes.iter().all(|_node| _node.has_arguments));

    Ok(())
}

#[test]
fn double_run() -> anyhow::Result<()> {
    let graph = Graph::from_yaml_file("../test_resources/test_graph.yml")?;
    let mut runtime = Runtime::default();
    let invoker = EmptyInvoker {};

    runtime.run(&graph, &invoker)?;

    let nodes = runtime.run(&graph, &invoker)?;
    assert!(nodes.nodes.iter().all(|node| node.has_arguments));
    assert!(!nodes.node_by_name("val1").unwrap().executed);
    assert!(!nodes.node_by_name("val2").unwrap().executed);
    assert!(!nodes.node_by_name("sum").unwrap().executed);
    assert!(!nodes.node_by_name("mult").unwrap().executed);
    assert!(nodes.node_by_name("print").unwrap().executed);

    Ok(())
}

#[test]
fn node_behavior_active_test() -> anyhow::Result<()> {
    let mut graph = Graph::from_yaml_file("../test_resources/test_graph.yml")?;
    let mut runtime = Runtime::default();
    let invoker = EmptyInvoker {};

    runtime.run(&graph, &invoker)?;

    graph.node_by_name_mut("val2").unwrap().behavior = NodeBehavior::Active;
    let nodes = runtime.run(&graph, &invoker)?;
    assert!(nodes.nodes.iter().all(|_node| _node.has_arguments));
    assert!(!nodes.node_by_name("val1").unwrap().executed);
    assert!(nodes.node_by_name("val2").unwrap().executed);
    assert!(!nodes.node_by_name("sum").unwrap().executed);
    assert!(nodes.node_by_name("mult").unwrap().executed);
    assert!(nodes.node_by_name("print").unwrap().executed);

    Ok(())
}

#[test]
fn edge_behavior_once_test() -> anyhow::Result<()> {
    let mut graph = Graph::from_yaml_file("../test_resources/test_graph.yml")?;
    let mut runtime = Runtime::default();
    let invoker = EmptyInvoker {};

    runtime.run(&graph, &invoker)?;

    graph.node_by_name_mut("mult").unwrap()
        .inputs.get_mut(1).unwrap()
        .binding.as_mut().unwrap()
        .behavior = BindingBehavior::Once;

    let nodes = runtime.run(&graph, &invoker)?;
    assert!(nodes.nodes.iter().all(|_node| _node.has_arguments));
    assert!(!nodes.node_by_name("val1").unwrap().executed);
    assert!(!nodes.node_by_name("val2").unwrap().executed);
    assert!(!nodes.node_by_name("sum").unwrap().executed);
    assert!(!nodes.node_by_name("mult").unwrap().executed);
    assert!(nodes.node_by_name("print").unwrap().executed);

    Ok(())
}

#[test]
fn edge_behavior_always_test() -> anyhow::Result<()> {
    let mut graph = Graph::from_yaml_file("../test_resources/test_graph.yml")?;
    let mut runtime = Runtime::default();
    let invoker = EmptyInvoker {};

    runtime.run(&graph, &invoker)?;

    graph.node_by_name_mut("sum").unwrap()
        .inputs.get_mut(0).unwrap()
        .binding.as_mut().unwrap()
        .behavior = BindingBehavior::Always;

    let nodes = runtime.run(&graph, &invoker)?;
    assert!(nodes.nodes.iter().all(|_node| _node.has_arguments));
    assert!(nodes.node_by_name("val1").unwrap().executed);
    assert!(!nodes.node_by_name("val2").unwrap().executed);
    assert!(nodes.node_by_name("sum").unwrap().executed);
    assert!(nodes.node_by_name("mult").unwrap().executed);
    assert!(nodes.node_by_name("print").unwrap().executed);

    Ok(())
}

#[test]
fn multiple_runs_with_various_modifications() -> anyhow::Result<()> {
    let mut graph = Graph::from_yaml_file("../test_resources/test_graph.yml")?;
    let mut runtime = Runtime::default();
    let invoker = EmptyInvoker {};

    runtime.run(&graph, &invoker)?;

    {
        let nodes = runtime.run(&graph, &invoker)?;
        assert!(!nodes.node_by_name("val1").unwrap().executed);
        assert!(!nodes.node_by_name("val2").unwrap().executed);
        assert!(!nodes.node_by_name("sum").unwrap().executed);
        assert!(!nodes.node_by_name("mult").unwrap().executed);
        assert!(nodes.node_by_name("print").unwrap().executed);
    }
    {
        graph.node_by_name_mut("val2").unwrap().behavior = NodeBehavior::Active;
        let nodes = runtime.run(&graph, &invoker)?;
        assert!(!nodes.node_by_name("val1").unwrap().executed);
        assert!(nodes.node_by_name("val2").unwrap().executed);
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
        assert!(!nodes.node_by_name("val1").unwrap().executed);
        assert!(!nodes.node_by_name("val2").unwrap().executed);
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
        assert!(nodes.node_by_name("val1").unwrap().executed);
        assert!(nodes.node_by_name("val2").unwrap().executed);
        assert!(nodes.node_by_name("sum").unwrap().executed);
        assert!(nodes.node_by_name("mult").unwrap().executed);
        assert!(nodes.node_by_name("print").unwrap().executed);
    }

    Ok(())
}
