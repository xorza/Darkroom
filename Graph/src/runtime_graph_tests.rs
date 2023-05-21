#[cfg(test)]
mod runtime_tests {
    use std::hint::black_box;
    use crate::graph::*;
    use crate::node::*;
    use crate::runtime_graph::RuntimeGraph;

    #[test]
    fn run() {
        let mut graph = Graph::from_json_file("./test_resources/test_graph.json");
        let mut runtime_graph = RuntimeGraph::new();

        {
            runtime_graph.run(&graph);
            assert_eq!(runtime_graph.nodes.iter().all(|_node| _node.should_execute), true);
            assert_eq!(runtime_graph.nodes.iter().all(|_node| _node.has_outputs), true);
        }
        {
            runtime_graph.run(&graph);
            assert_eq!(runtime_graph.nodes.iter().all(|_node| _node.has_outputs), true);
            assert_eq!(runtime_graph.node_by_id(0).unwrap().should_execute, false);
            assert_eq!(runtime_graph.node_by_id(1).unwrap().should_execute, false);
            assert_eq!(runtime_graph.node_by_id(2).unwrap().should_execute, false);
            assert_eq!(runtime_graph.node_by_id(3).unwrap().should_execute, false);
            assert_eq!(runtime_graph.node_by_id(4).unwrap().should_execute, true);
        }
        {
            graph.node_by_id_mut(1).unwrap().behavior = NodeBehavior::Active;
            runtime_graph.run(&graph);
            assert_eq!(runtime_graph.nodes.iter().all(|_node| _node.has_outputs), true);
            assert_eq!(runtime_graph.node_by_id(0).unwrap().should_execute, false);
            assert_eq!(runtime_graph.node_by_id(1).unwrap().should_execute, true);
            assert_eq!(runtime_graph.node_by_id(2).unwrap().should_execute, false);
            assert_eq!(runtime_graph.node_by_id(3).unwrap().should_execute, true);
            assert_eq!(runtime_graph.node_by_id(4).unwrap().should_execute, true);
        }
        {
            graph.edge_by_input_id_mut(3).unwrap().behavior = EdgeBehavior::Once;
            runtime_graph.run(&graph);
            assert_eq!(runtime_graph.nodes.iter().all(|_node| _node.has_outputs), true);
            assert_eq!(runtime_graph.node_by_id(0).unwrap().should_execute, false);
            assert_eq!(runtime_graph.node_by_id(1).unwrap().should_execute, false);
            assert_eq!(runtime_graph.node_by_id(2).unwrap().should_execute, false);
            assert_eq!(runtime_graph.node_by_id(3).unwrap().should_execute, false);
            assert_eq!(runtime_graph.node_by_id(4).unwrap().should_execute, true);
        }
        {
            graph.edge_by_input_id_mut(1).unwrap().behavior = EdgeBehavior::Always;
            runtime_graph.run(&graph);
            assert_eq!(runtime_graph.nodes.iter().all(|_node| _node.has_outputs), true);
            assert_eq!(runtime_graph.node_by_id(0).unwrap().should_execute, false);
            assert_eq!(runtime_graph.node_by_id(1).unwrap().should_execute, true);
            assert_eq!(runtime_graph.node_by_id(2).unwrap().should_execute, true);
            assert_eq!(runtime_graph.node_by_id(3).unwrap().should_execute, true);
            assert_eq!(runtime_graph.node_by_id(4).unwrap().should_execute, true);
        }

        black_box(());
    }
}
