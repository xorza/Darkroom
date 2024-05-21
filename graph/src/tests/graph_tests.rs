use std::hint::black_box;

use crate::data::{ StaticValue};
use crate::graph::*;

#[test]
fn graph_to_yaml() -> anyhow::Result<()> {
    let mut graph = Graph::default();
    let mut node1 = Node::new();

    node1.inputs.push(Input {
        binding: Binding::Const,
        const_value: Some(StaticValue::Int(55)),
    });
    let mut node2 = Node::new();
    node2.inputs.push(Input {
        binding: Binding::Output(OutputBinding {
            output_node_id: node1.id(),
            output_index: 0,
        }),
        const_value: None,
    });

    graph.add_node(node1);
    graph.add_node(node2);

    let _yaml: String = graph.to_yaml()?;

    Ok(())
}

#[test]
fn graph_from_yaml() -> anyhow::Result<()> {
    let graph = Graph::from_yaml_file("../test_resources/test_graph.yml")?;
    let yaml: String = graph.to_yaml()?;
    let graph = Graph::from_yaml(&yaml)?;
    black_box(graph);

    Ok(())
}

#[test]
fn node_remove_test() -> anyhow::Result<()> {
    let mut graph = Graph::from_yaml_file("../test_resources/test_graph.yml")?;

    let node_id = graph.node_by_name("sum").unwrap().id();
    graph.remove_node_by_id(node_id);

    assert!(graph.node_by_name("sum").is_none());
    assert_eq!(graph.nodes().len(), 4);

    for input in graph.nodes().iter().flat_map(|node| node.inputs.iter()) {
        if let Some(binding) = input.binding.as_output_binding() {
            assert_ne!(binding.output_node_id, node_id);
        }
    }

    Ok(())
}
