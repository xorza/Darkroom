use crate::function::FuncLib;
use crate::graph::*;
use crate::runtime_graph::RuntimeGraph;

#[test]
fn simple_run() -> anyhow::Result<()> {
    let graph = Graph::from_yaml_file("../test_resources/test_graph.yml")?;
    let func_lib = FuncLib::from_yaml_file("../test_resources/test_funcs.yml")?;

    let get_b_node_id = graph.node_by_name("get_b").unwrap().id;

    let mut runtime_graph = RuntimeGraph::new(&graph, &func_lib);
    runtime_graph.next(&graph);

    assert_eq!(runtime_graph.nodes.len(), 5);
    assert_eq!(
        runtime_graph
            .node_by_id(get_b_node_id)
            .unwrap()
            .total_binding_count,
        2
    );
    assert!(runtime_graph
        .nodes
        .iter()
        .all(|r_node| r_node.should_invoke));
    assert!(runtime_graph
        .nodes
        .iter()
        .all(|r_node| !r_node.has_missing_inputs));

    let _yaml = serde_yaml::to_string(&runtime_graph)?;

    Ok(())
}

#[test]
fn empty_run() -> anyhow::Result<()> {
    let graph = Graph::from_yaml_file("../test_resources/test_graph.yml")?;
    let func_lib = FuncLib::from_yaml_file("../test_resources/test_funcs.yml")?;

    let get_b_node_id = graph.node_by_name("get_b").unwrap().id;

    let mut runtime_graph = RuntimeGraph::new(&graph, &func_lib);
    runtime_graph.next(&graph);

    assert_eq!(runtime_graph.nodes.len(), 5);
    assert_eq!(
        runtime_graph
            .node_by_id(get_b_node_id)
            .unwrap()
            .total_binding_count,
        2
    );

    runtime_graph.next(&graph);

    assert_eq!(runtime_graph.nodes.len(), 5);
    assert_eq!(
        runtime_graph
            .node_by_id(get_b_node_id)
            .unwrap()
            .total_binding_count,
        2
    );

    Ok(())
}

#[test]
fn missing_input() -> anyhow::Result<()> {
    let mut graph = Graph::from_yaml_file("../test_resources/test_graph.yml")?;
    let func_lib = FuncLib::from_yaml_file("../test_resources/test_funcs.yml")?;

    let get_b_node_id = graph.node_by_name("get_b").unwrap().id;
    let sum_node_id = graph.node_by_name("sum").unwrap().id;
    let mult_node_id = graph.node_by_name("mult").unwrap().id;
    let print_node_id = graph.node_by_name("print").unwrap().id;

    graph.node_by_name_mut("sum").unwrap().inputs[0].binding = Binding::None;

    let mut runtime_graph = RuntimeGraph::new(&graph, &func_lib);
    runtime_graph.next(&graph);

    assert_eq!(runtime_graph.nodes.len(), 4);
    assert_eq!(
        runtime_graph
            .node_by_id(get_b_node_id)
            .unwrap()
            .total_binding_count,
        2
    );

    assert!(
        runtime_graph
            .node_by_id(get_b_node_id)
            .unwrap()
            .should_invoke
    );
    assert!(!runtime_graph.node_by_id(sum_node_id).unwrap().should_invoke);
    assert!(
        !runtime_graph
            .node_by_id(mult_node_id)
            .unwrap()
            .should_invoke
    );
    assert!(
        !runtime_graph
            .node_by_id(print_node_id)
            .unwrap()
            .should_invoke
    );

    assert!(
        !runtime_graph
            .node_by_id(get_b_node_id)
            .unwrap()
            .has_missing_inputs
    );
    assert!(
        runtime_graph
            .node_by_id(sum_node_id)
            .unwrap()
            .has_missing_inputs
    );
    assert!(
        runtime_graph
            .node_by_id(mult_node_id)
            .unwrap()
            .has_missing_inputs
    );
    assert!(
        runtime_graph
            .node_by_id(print_node_id)
            .unwrap()
            .has_missing_inputs
    );

    let _yaml = serde_yaml::to_string(&runtime_graph)?;

    Ok(())
}
