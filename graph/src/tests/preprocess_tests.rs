use crate::data::DataType;
use crate::function::{Func, FuncId, FuncLib, InputInfo, OutputInfo};
use crate::graph::*;
use crate::runtime_graph::RuntimeGraph;
use std::str::FromStr;

fn create_func_lib() -> FuncLib {
    [
        Func {
            id: FuncId::from_str("432b9bf1-f478-476c-a9c9-9a6e190124fc").unwrap(),
            name: "mult".to_string(),
            category: "".to_string(),
            behavior: FuncBehavior::Passive,
            is_output: false,
            inputs: vec![
                InputInfo {
                    name: "A".to_string(),
                    is_required: true,
                    data_type: DataType::Int,
                    default_value: None,
                    variants: vec![],
                },
                InputInfo {
                    name: "B".to_string(),
                    is_required: true,
                    data_type: DataType::Int,
                    default_value: None,
                    variants: vec![],
                },
            ],
            outputs: vec![OutputInfo {
                name: "Prod".to_string(),
                data_type: DataType::Int,
            }],
            events: vec![],
        },
        Func {
            id: FuncId::from_str("d4d27137-5a14-437a-8bb5-b2f7be0941a2").unwrap(),
            name: "get_a".to_string(),
            category: "".to_string(),
            behavior: FuncBehavior::Active,
            is_output: false,
            inputs: vec![],
            outputs: vec![OutputInfo {
                name: "Int32 Value".to_string(),
                data_type: DataType::Int,
            }],
            events: vec![],
        },
        Func {
            id: FuncId::from_str("a937baff-822d-48fd-9154-58751539b59b").unwrap(),
            name: "get_b".to_string(),
            category: "".to_string(),
            behavior: FuncBehavior::Passive,
            is_output: false,
            inputs: vec![],
            outputs: vec![OutputInfo {
                name: "Int32 Value".to_string(),
                data_type: DataType::Int,
            }],
            events: vec![],
        },
        Func {
            id: FuncId::from_str("2d3b389d-7b58-44d9-b3d1-a595765b21a5").unwrap(),
            name: "sum".to_string(),
            category: "".to_string(),
            behavior: FuncBehavior::Passive,
            is_output: false,
            inputs: vec![
                InputInfo {
                    name: "A".to_string(),
                    is_required: true,
                    data_type: DataType::Int,
                    default_value: None,
                    variants: vec![],
                },
                InputInfo {
                    name: "B".to_string(),
                    is_required: true,
                    data_type: DataType::Int,
                    default_value: None,
                    variants: vec![],
                },
            ],
            outputs: vec![OutputInfo {
                name: "Sum".to_string(),
                data_type: DataType::Int,
            }],
            events: vec![],
        },
        Func {
            id: FuncId::from_str("f22cd316-1cdf-4a80-b86c-1277acd1408a").unwrap(),
            name: "print".to_string(),
            category: "".to_string(),
            behavior: FuncBehavior::Passive,
            is_output: false,
            inputs: vec![InputInfo {
                name: "message".to_string(),
                is_required: true,
                data_type: DataType::Int,
                default_value: None,
                variants: vec![],
            }],
            outputs: vec![],
            events: vec![],
        },
    ]
    .into()
}

#[test]
fn simple_run() -> anyhow::Result<()> {
    let graph = Graph::from_yaml_file("../test_resources/test_graph.yml")?;
    let func_lib = create_func_lib();
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
fn empty_run() {
    let graph = Graph::from_yaml_file("../test_resources/test_graph.yml").unwrap();
    let func_lib = create_func_lib();
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
}

#[test]
fn missing_input() -> anyhow::Result<()> {
    let mut graph = Graph::from_yaml_file("../test_resources/test_graph.yml")?;
    let func_lib = create_func_lib();

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
