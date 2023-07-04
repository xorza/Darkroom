use std::str::FromStr;

use uuid::Uuid;

use crate::data::Value;
use crate::graph::*;
use crate::invoke::{InvokeArgs, Invoker, LambdaInvoker};
use crate::runtime::{Runtime, RuntimeInfo};

struct EmptyInvoker {}

impl Invoker for EmptyInvoker {
    fn call(&self, _: Uuid, _: Uuid, _: &InvokeArgs, _: &mut InvokeArgs) -> anyhow::Result<()> {
        Ok(())
    }
}


#[test]
fn simple_run() -> anyhow::Result<()> {
    let graph = Graph::from_yaml_file("../test_resources/test_graph.yml")?;
    let mut runtime = Runtime::default();

    let nodes = runtime.run(&graph, &RuntimeInfo::default())?;
    assert!(nodes.nodes.iter().all(|_node| _node.should_execute));
    assert!(nodes.nodes.iter().all(|_node| _node.has_outputs));

    Ok(())
}

#[test]
fn double_run() -> anyhow::Result<()> {
    let graph = Graph::from_yaml_file("../test_resources/test_graph.yml")?;
    let mut runtime = Runtime::default();

    let runtime_info = runtime.run(&graph, &RuntimeInfo::default())?;

    let runtime_info = runtime.run(&graph, &runtime_info)?;
    assert!(runtime_info.nodes.iter().all(|node| node.has_outputs));
    assert!(!runtime_info.node_by_name("val1").unwrap().should_execute);
    assert!(!runtime_info.node_by_name("val2").unwrap().should_execute);
    assert!(!runtime_info.node_by_name("sum").unwrap().should_execute);
    assert!(!runtime_info.node_by_name("mult").unwrap().should_execute);
    assert!(runtime_info.node_by_name("print").unwrap().should_execute);

    Ok(())
}

#[test]
fn node_behavior_active_test() -> anyhow::Result<()> {
    let mut graph = Graph::from_yaml_file("../test_resources/test_graph.yml")?;
    let mut runtime = Runtime::default();

    let runtime_info = runtime.run(&graph, &RuntimeInfo::default())?;

    graph.node_by_name_mut("val2").unwrap().behavior = FunctionBehavior::Active;
    let runtime_info = runtime.run(&graph, &runtime_info)?;
    assert!(runtime_info.nodes.iter().all(|_node| _node.has_outputs));
    assert!(!runtime_info.node_by_name("val1").unwrap().should_execute);
    assert!(runtime_info.node_by_name("val2").unwrap().should_execute);
    assert!(!runtime_info.node_by_name("sum").unwrap().should_execute);
    assert!(runtime_info.node_by_name("mult").unwrap().should_execute);
    assert!(runtime_info.node_by_name("print").unwrap().should_execute);

    Ok(())
}

#[test]
fn edge_behavior_once_test() -> anyhow::Result<()> {
    let mut graph = Graph::from_yaml_file("../test_resources/test_graph.yml")?;
    let mut runtime = Runtime::default();

    let runtime_info = runtime.run(&graph, &RuntimeInfo::default())?;

    graph.node_by_name_mut("mult").unwrap()
        .inputs.get_mut(1).unwrap()
        .binding.as_mut().unwrap()
        .behavior = BindingBehavior::Once;

    let runtime_info = runtime.run(&graph, &runtime_info)?;
    assert!(runtime_info.nodes.iter().all(|_node| _node.has_outputs));
    assert!(!runtime_info.node_by_name("val1").unwrap().should_execute);
    assert!(!runtime_info.node_by_name("val2").unwrap().should_execute);
    assert!(!runtime_info.node_by_name("sum").unwrap().should_execute);
    assert!(!runtime_info.node_by_name("mult").unwrap().should_execute);
    assert!(runtime_info.node_by_name("print").unwrap().should_execute);

    Ok(())
}

#[test]
fn edge_behavior_always_test() -> anyhow::Result<()> {
    let mut graph = Graph::from_yaml_file("../test_resources/test_graph.yml")?;
    let mut runtime = Runtime::default();

    let runtime_info = runtime.run(&graph, &RuntimeInfo::default())?;

    graph.node_by_name_mut("sum").unwrap()
        .inputs.get_mut(0).unwrap()
        .binding.as_mut().unwrap()
        .behavior = BindingBehavior::Always;

    let runtime_info = runtime.run(&graph, &runtime_info)?;
    assert!(runtime_info.nodes.iter().all(|_node| _node.has_outputs));
    assert!(runtime_info.node_by_name("val1").unwrap().should_execute);
    assert!(!runtime_info.node_by_name("val2").unwrap().should_execute);
    assert!(runtime_info.node_by_name("sum").unwrap().should_execute);
    assert!(runtime_info.node_by_name("mult").unwrap().should_execute);
    assert!(runtime_info.node_by_name("print").unwrap().should_execute);

    Ok(())
}

#[test]
fn multiple_runs_with_various_modifications() -> anyhow::Result<()> {
    let mut graph = Graph::from_yaml_file("../test_resources/test_graph.yml")?;
    let mut runtime = Runtime::default();

    let runtime_info = runtime.run(&graph, &RuntimeInfo::default())?;
    assert_eq!(runtime_info.node_by_name("sum").unwrap().outputs[0].connection_count, 1);

    let runtime_info = runtime.run(&graph, &runtime_info)?;
    assert!(!runtime_info.node_by_name("val1").unwrap().should_execute);
    assert!(!runtime_info.node_by_name("val2").unwrap().should_execute);
    assert!(!runtime_info.node_by_name("sum").unwrap().should_execute);
    assert!(!runtime_info.node_by_name("mult").unwrap().should_execute);
    assert!(runtime_info.node_by_name("print").unwrap().should_execute);
    assert_eq!(runtime_info.node_by_name("sum").unwrap().outputs[0].connection_count, 0);

    graph.node_by_name_mut("val2").unwrap().behavior = FunctionBehavior::Active;
    let runtime_info = runtime.run(&graph, &runtime_info)?;
    assert!(!runtime_info.node_by_name("val1").unwrap().should_execute);
    assert!(runtime_info.node_by_name("val2").unwrap().should_execute);
    assert!(!runtime_info.node_by_name("sum").unwrap().should_execute);
    assert!(runtime_info.node_by_name("mult").unwrap().should_execute);
    assert!(runtime_info.node_by_name("print").unwrap().should_execute);

    graph.node_by_name_mut("mult").unwrap()
        .inputs.get_mut(1).unwrap()
        .binding.as_mut().unwrap()
        .behavior = BindingBehavior::Once;
    let runtime_info = runtime.run(&graph, &runtime_info)?;
    assert!(!runtime_info.node_by_name("val1").unwrap().should_execute);
    assert!(!runtime_info.node_by_name("val2").unwrap().should_execute);
    assert!(!runtime_info.node_by_name("sum").unwrap().should_execute);
    assert!(!runtime_info.node_by_name("mult").unwrap().should_execute);
    assert!(runtime_info.node_by_name("print").unwrap().should_execute);

    graph.node_by_name_mut("sum").unwrap()
        .inputs.get_mut(1).unwrap()
        .binding.as_mut().unwrap()
        .behavior = BindingBehavior::Always;
    let runtime_info = runtime.run(&graph, &runtime_info)?;
    assert!(runtime_info.nodes.iter().all(|_node| _node.has_outputs));
    assert!(runtime_info.node_by_name("val1").unwrap().should_execute);
    assert!(runtime_info.node_by_name("val2").unwrap().should_execute);
    assert!(runtime_info.node_by_name("sum").unwrap().should_execute);
    assert!(runtime_info.node_by_name("mult").unwrap().should_execute);
    assert!(runtime_info.node_by_name("print").unwrap().should_execute);


    Ok(())
}
