use std::hint::black_box;

use crate::graph::*;

#[test]
fn graph_from_yaml() -> anyhow::Result<()> {
    let graph = Graph::from_yaml_file("./test_resources/test_graph.yml")?;
    let yaml: String = graph.to_yaml()?;
    let graph = Graph::from_yaml(&yaml)?;
    black_box(graph);

    Ok(())
}

#[test]
fn node_remove_test() -> anyhow::Result<()> {
    let mut graph = Graph::from_yaml_file("./test_resources/test_graph.yml")?;

    let node_id = graph.node_by_name("sum").unwrap().id();
    graph.remove_node_by_id(node_id);

    assert!(graph.node_by_name("sum").is_none());
    assert_eq!(graph.nodes().len(), 4);

    for input in graph.nodes().iter().flat_map(|node| node.inputs.iter()) {
        if let Some(binding) = input.binding.as_ref() {
            assert_ne!(binding.output_node_id(), node_id);
        }
    }

    Ok(())
}

#[test]
fn subgraph_from_yaml() -> anyhow::Result<()> {
    let graph = Graph::from_yaml_file("./test_resources/test_subgraph.yml")?;
    let _yaml: String = graph.to_yaml()?;

    assert_eq!(graph.subgraphs().len(), 1);
    let circle = graph.subgraphs()[0].clone();
    assert_eq!(circle.name, "circle");
    assert_eq!(circle.inputs.len(), 1);
    assert_eq!(circle.outputs.len(), 2);

    Ok(())
}

#[test]
fn test_graph_validation() -> anyhow::Result<()> {
    Ok(())
}
