use crate::function::FunctionId;
use crate::graph::*;
use crate::invoke_context::{InvokeArgs, InvokeCache, Invoker};
use crate::runtime_graph::RuntimeGraph;

struct EmptyInvoker {}

impl Invoker for EmptyInvoker {
    fn invoke(&self,
              _function_id: FunctionId,
              _cache: &mut InvokeCache,
              _inputs: &mut InvokeArgs,
              _outputs: &mut InvokeArgs)
        -> anyhow::Result<()> {
        Ok(())
    }
}


#[test]
fn simple_run() -> anyhow::Result<()> {
    let graph = Graph::from_yaml_file("../test_resources/test_graph.yml")?;

    let mut runtime_graph = RuntimeGraph::from(&graph);
    runtime_graph.next(&graph);

    assert_eq!(runtime_graph.nodes.len(), 5);
    assert_eq!(runtime_graph.node_by_name("get_b").unwrap().total_binding_count, 2);
    assert!(runtime_graph.nodes.iter().all(|r_node| r_node.should_invoke));
    assert!(runtime_graph.nodes.iter().all(|r_node| !r_node.has_missing_inputs));

    let _yaml = serde_yaml::to_string(&runtime_graph)?;

    Ok(())
}

#[test]
fn empty_run() {
    let graph = Graph::from_yaml_file("../test_resources/test_graph.yml").unwrap();
    let mut runtime_graph = RuntimeGraph::from(&graph);
    runtime_graph.next(&graph);

    assert_eq!(runtime_graph.nodes.len(), 5);
    assert_eq!(runtime_graph.node_by_name("get_b").unwrap().total_binding_count, 2);

    runtime_graph.next(&graph);

    assert_eq!(runtime_graph.nodes.len(), 5);
    assert_eq!(runtime_graph.node_by_name("get_b").unwrap().total_binding_count, 2);
}

#[test]
fn missing_input() -> anyhow::Result<()> {
    let mut graph = Graph::from_yaml_file("../test_resources/test_graph.yml")?;
    graph.node_by_name_mut("sum").unwrap()
        .inputs[0].binding = Binding::None;

    let mut runtime_graph = RuntimeGraph::from(&graph);
    runtime_graph.next(&graph);

    assert_eq!(runtime_graph.nodes.len(), 4);
    assert_eq!(runtime_graph.node_by_name("get_b").unwrap().total_binding_count, 2);

    assert!(runtime_graph.node_by_name("get_b").unwrap().should_invoke);
    assert!(!runtime_graph.node_by_name("sum").unwrap().should_invoke);
    assert!(!runtime_graph.node_by_name("mult").unwrap().should_invoke);
    assert!(!runtime_graph.node_by_name("print").unwrap().should_invoke);

    assert!(!runtime_graph.node_by_name("get_b").unwrap().has_missing_inputs);
    assert!(runtime_graph.node_by_name("sum").unwrap().has_missing_inputs);
    assert!(runtime_graph.node_by_name("mult").unwrap().has_missing_inputs);
    assert!(runtime_graph.node_by_name("print").unwrap().has_missing_inputs);

    let _yaml = serde_yaml::to_string(&runtime_graph)?;

    Ok(())
}
