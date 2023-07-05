use std::any::Any;
use std::str::FromStr;

use uuid::Uuid;

use crate::compute::{Compute, DynamicContext, InvokeArgs, LambdaCompute};
use crate::data::Value;
use crate::graph::*;
use crate::preprocess::{Preprocess, PreprocessInfo};

struct EmptyInvoker {}

impl Compute for EmptyInvoker {
    fn invoke(&self,
              _function_id: Uuid,
              _ctx: &mut DynamicContext,
              _inputs: &InvokeArgs,
              _outputs: &mut InvokeArgs)
              -> anyhow::Result<()> {
        Ok(())
    }
}


#[test]
fn simple_run() -> anyhow::Result<()> {
    let graph = Graph::from_yaml_file("../test_resources/test_graph.yml")?;
    let runtime = Preprocess::default();

    let nodes = runtime.run(&graph, &PreprocessInfo::default())?;
    assert!(nodes.nodes.iter().all(|_node| _node.should_execute));
    assert!(nodes.nodes.iter().all(|_node| _node.has_outputs));

    Ok(())
}

#[test]
fn double_run() -> anyhow::Result<()> {
    let graph = Graph::from_yaml_file("../test_resources/test_graph.yml")?;
    let runtime = Preprocess::default();

    let preprocess_info = runtime.run(&graph, &PreprocessInfo::default())?;

    let preprocess_info = runtime.run(&graph, &preprocess_info)?;
    assert!(preprocess_info.nodes.iter().all(|node| node.has_outputs));
    assert!(!preprocess_info.node_by_name("val1").unwrap().should_execute);
    assert!(!preprocess_info.node_by_name("val2").unwrap().should_execute);
    assert!(!preprocess_info.node_by_name("sum").unwrap().should_execute);
    assert!(!preprocess_info.node_by_name("mult").unwrap().should_execute);
    assert!(preprocess_info.node_by_name("print").unwrap().should_execute);

    Ok(())
}

#[test]
fn node_behavior_active_test() -> anyhow::Result<()> {
    let mut graph = Graph::from_yaml_file("../test_resources/test_graph.yml")?;
    let runtime = Preprocess::default();

    let preprocess_info = runtime.run(&graph, &PreprocessInfo::default())?;

    graph.node_by_name_mut("val2").unwrap().behavior = FunctionBehavior::Active;
    let preprocess_info = runtime.run(&graph, &preprocess_info)?;
    assert!(preprocess_info.nodes.iter().all(|_node| _node.has_outputs));
    assert!(!preprocess_info.node_by_name("val1").unwrap().should_execute);
    assert!(preprocess_info.node_by_name("val2").unwrap().should_execute);
    assert!(!preprocess_info.node_by_name("sum").unwrap().should_execute);
    assert!(preprocess_info.node_by_name("mult").unwrap().should_execute);
    assert!(preprocess_info.node_by_name("print").unwrap().should_execute);

    Ok(())
}

#[test]
fn edge_behavior_once_test() -> anyhow::Result<()> {
    let mut graph = Graph::from_yaml_file("../test_resources/test_graph.yml")?;
    let runtime = Preprocess::default();

    let preprocess_info = runtime.run(&graph, &PreprocessInfo::default())?;

    graph.node_by_name_mut("mult").unwrap()
        .inputs.get_mut(1).unwrap()
        .binding.as_output_binding_mut().unwrap()
        .behavior = BindingBehavior::Once;

    let preprocess_info = runtime.run(&graph, &preprocess_info)?;
    assert!(preprocess_info.nodes.iter().all(|_node| _node.has_outputs));
    assert!(!preprocess_info.node_by_name("val1").unwrap().should_execute);
    assert!(!preprocess_info.node_by_name("val2").unwrap().should_execute);
    assert!(!preprocess_info.node_by_name("sum").unwrap().should_execute);
    assert!(!preprocess_info.node_by_name("mult").unwrap().should_execute);
    assert!(preprocess_info.node_by_name("print").unwrap().should_execute);

    Ok(())
}

#[test]
fn edge_behavior_always_test() -> anyhow::Result<()> {
    let mut graph = Graph::from_yaml_file("../test_resources/test_graph.yml")?;
    let runtime = Preprocess::default();

    let preprocess_info = runtime.run(&graph, &PreprocessInfo::default())?;

    graph.node_by_name_mut("sum").unwrap()
        .inputs.get_mut(0).unwrap()
        .binding.as_output_binding_mut().unwrap()
        .behavior = BindingBehavior::Always;

    let preprocess_info = runtime.run(&graph, &preprocess_info)?;
    assert!(preprocess_info.nodes.iter().all(|_node| _node.has_outputs));
    assert!(preprocess_info.node_by_name("val1").unwrap().should_execute);
    assert!(!preprocess_info.node_by_name("val2").unwrap().should_execute);
    assert!(preprocess_info.node_by_name("sum").unwrap().should_execute);
    assert!(preprocess_info.node_by_name("mult").unwrap().should_execute);
    assert!(preprocess_info.node_by_name("print").unwrap().should_execute);
    assert_eq!(preprocess_info.node_by_name("val1").unwrap().outputs[0].connection_count, 1);
    assert_eq!(preprocess_info.node_by_name("val2").unwrap().outputs[0].connection_count, 0);

    Ok(())
}

#[test]
fn multiple_runs_with_various_modifications() -> anyhow::Result<()> {
    let mut graph = Graph::from_yaml_file("../test_resources/test_graph.yml")?;
    let runtime = Preprocess::default();

    let preprocess_info = runtime.run(&graph, &PreprocessInfo::default())?;
    assert_eq!(preprocess_info.node_by_name("sum").unwrap().outputs[0].connection_count, 1);

    let preprocess_info = runtime.run(&graph, &preprocess_info)?;
    assert!(!preprocess_info.node_by_name("val1").unwrap().should_execute);
    assert!(!preprocess_info.node_by_name("val2").unwrap().should_execute);
    assert!(!preprocess_info.node_by_name("sum").unwrap().should_execute);
    assert!(!preprocess_info.node_by_name("mult").unwrap().should_execute);
    assert!(preprocess_info.node_by_name("print").unwrap().should_execute);
    assert_eq!(preprocess_info.node_by_name("sum").unwrap().outputs[0].connection_count, 0);

    graph.node_by_name_mut("val2").unwrap().behavior = FunctionBehavior::Active;
    let preprocess_info = runtime.run(&graph, &preprocess_info)?;
    assert!(!preprocess_info.node_by_name("val1").unwrap().should_execute);
    assert!(preprocess_info.node_by_name("val2").unwrap().should_execute);
    assert!(!preprocess_info.node_by_name("sum").unwrap().should_execute);
    assert!(preprocess_info.node_by_name("mult").unwrap().should_execute);
    assert!(preprocess_info.node_by_name("print").unwrap().should_execute);

    graph.node_by_name_mut("mult").unwrap()
        .inputs.get_mut(1).unwrap()
        .binding.as_output_binding_mut().unwrap()
        .behavior = BindingBehavior::Once;
    let preprocess_info = runtime.run(&graph, &preprocess_info)?;
    assert!(!preprocess_info.node_by_name("val1").unwrap().should_execute);
    assert!(!preprocess_info.node_by_name("val2").unwrap().should_execute);
    assert!(!preprocess_info.node_by_name("sum").unwrap().should_execute);
    assert!(!preprocess_info.node_by_name("mult").unwrap().should_execute);
    assert!(preprocess_info.node_by_name("print").unwrap().should_execute);

    graph.node_by_name_mut("sum").unwrap()
        .inputs.get_mut(1).unwrap()
        .binding.as_output_binding_mut().unwrap()
        .behavior = BindingBehavior::Always;
    let preprocess_info = runtime.run(&graph, &preprocess_info)?;
    assert!(preprocess_info.nodes.iter().all(|_node| _node.has_outputs));
    assert!(preprocess_info.node_by_name("val1").unwrap().should_execute);
    assert!(preprocess_info.node_by_name("val2").unwrap().should_execute);
    assert!(preprocess_info.node_by_name("sum").unwrap().should_execute);
    assert!(preprocess_info.node_by_name("mult").unwrap().should_execute);
    assert!(preprocess_info.node_by_name("print").unwrap().should_execute);


    Ok(())
}
