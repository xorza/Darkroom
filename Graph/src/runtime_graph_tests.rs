#[cfg(test)]
mod runtime_tests {
    use crate::graph::*;
    use crate::node::*;
    use crate::runtime_graph::RuntimeGraph;

    #[test]
    fn simple_run() {
        let graph = Graph::from_json_file("./test_resources/test_graph.json");
        let mut runtime_graph = RuntimeGraph::new();

        runtime_graph.run(&graph);
        assert_eq!(runtime_graph.nodes.iter().all(|_node| _node.should_execute), true);
        assert_eq!(runtime_graph.nodes.iter().all(|_node| _node.has_outputs), true);
    }

    #[test]
    fn double_run() {
        let graph = Graph::from_json_file("./test_resources/test_graph.json");
        let mut runtime_graph = RuntimeGraph::new();

        runtime_graph.run(&graph);
        assert_eq!(runtime_graph.nodes.iter().all(|_node| _node.should_execute), true);
        assert_eq!(runtime_graph.nodes.iter().all(|_node| _node.has_outputs), true);

        runtime_graph.run(&graph);
        assert_eq!(runtime_graph.nodes.iter().all(|_node| _node.has_outputs), true);
        assert_eq!(runtime_graph.node_by_id(1).unwrap().should_execute, false);
        assert_eq!(runtime_graph.node_by_id(2).unwrap().should_execute, false);
        assert_eq!(runtime_graph.node_by_id(3).unwrap().should_execute, false);
        assert_eq!(runtime_graph.node_by_id(4).unwrap().should_execute, false);
        assert_eq!(runtime_graph.node_by_id(5).unwrap().should_execute, true);
    }

    #[test]
    fn node_behavior_active_test() {
        let mut graph = Graph::from_json_file("./test_resources/test_graph.json");
        let mut runtime_graph = RuntimeGraph::new();

        runtime_graph.run(&graph);
        assert_eq!(runtime_graph.nodes.iter().all(|_node| _node.should_execute), true);
        assert_eq!(runtime_graph.nodes.iter().all(|_node| _node.has_outputs), true);

        graph.node_by_id_mut(2).unwrap().behavior = NodeBehavior::Active;
        runtime_graph.run(&graph);
        assert_eq!(runtime_graph.nodes.iter().all(|_node| _node.has_outputs), true);
        assert_eq!(runtime_graph.node_by_id(1).unwrap().should_execute, false);
        assert_eq!(runtime_graph.node_by_id(2).unwrap().should_execute, true);
        assert_eq!(runtime_graph.node_by_id(3).unwrap().should_execute, false);
        assert_eq!(runtime_graph.node_by_id(4).unwrap().should_execute, true);
        assert_eq!(runtime_graph.node_by_id(5).unwrap().should_execute, true);
    }

    #[test]
    fn edge_behavior_once_test() {
        let mut graph = Graph::from_json_file("./test_resources/test_graph.json");
        let mut runtime_graph = RuntimeGraph::new();

        runtime_graph.run(&graph);
        assert_eq!(runtime_graph.nodes.iter().all(|_node| _node.should_execute), true);
        assert_eq!(runtime_graph.nodes.iter().all(|_node| _node.has_outputs), true);

        graph.edge_by_input_id_mut(13).unwrap().behavior = EdgeBehavior::Once;
        runtime_graph.run(&graph);
        assert_eq!(runtime_graph.nodes.iter().all(|_node| _node.has_outputs), true);
        assert_eq!(runtime_graph.node_by_id(1).unwrap().should_execute, false);
        assert_eq!(runtime_graph.node_by_id(2).unwrap().should_execute, false);
        assert_eq!(runtime_graph.node_by_id(3).unwrap().should_execute, false);
        assert_eq!(runtime_graph.node_by_id(4).unwrap().should_execute, false);
        assert_eq!(runtime_graph.node_by_id(5).unwrap().should_execute, true);
    }

    #[test]
    fn edge_behavior_always_test() {
        let mut graph = Graph::from_json_file("./test_resources/test_graph.json");
        let mut runtime_graph = RuntimeGraph::new();

        runtime_graph.run(&graph);
        assert_eq!(runtime_graph.nodes.iter().all(|_node| _node.should_execute), true);
        assert_eq!(runtime_graph.nodes.iter().all(|_node| _node.has_outputs), true);

        graph.edge_by_input_id_mut(10).unwrap().behavior = EdgeBehavior::Always;
        runtime_graph.run(&graph);
        assert_eq!(runtime_graph.nodes.iter().all(|_node| _node.has_outputs), true);
        assert_eq!(runtime_graph.node_by_id(1).unwrap().should_execute, true);
        assert_eq!(runtime_graph.node_by_id(2).unwrap().should_execute, false);
        assert_eq!(runtime_graph.node_by_id(3).unwrap().should_execute, true);
        assert_eq!(runtime_graph.node_by_id(4).unwrap().should_execute, true);
        assert_eq!(runtime_graph.node_by_id(5).unwrap().should_execute, true);
    }

    #[test]
    fn multiple_runs_with_various_modifications() {
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
            assert_eq!(runtime_graph.node_by_id(1).unwrap().should_execute, false);
            assert_eq!(runtime_graph.node_by_id(2).unwrap().should_execute, false);
            assert_eq!(runtime_graph.node_by_id(3).unwrap().should_execute, false);
            assert_eq!(runtime_graph.node_by_id(4).unwrap().should_execute, false);
            assert_eq!(runtime_graph.node_by_id(5).unwrap().should_execute, true);
        }
        {
            graph.node_by_id_mut(2).unwrap().behavior = NodeBehavior::Active;
            runtime_graph.run(&graph);
            assert_eq!(runtime_graph.nodes.iter().all(|_node| _node.has_outputs), true);
            assert_eq!(runtime_graph.node_by_id(1).unwrap().should_execute, false);
            assert_eq!(runtime_graph.node_by_id(2).unwrap().should_execute, true);
            assert_eq!(runtime_graph.node_by_id(3).unwrap().should_execute, false);
            assert_eq!(runtime_graph.node_by_id(4).unwrap().should_execute, true);
            assert_eq!(runtime_graph.node_by_id(5).unwrap().should_execute, true);
        }
        {
            graph.edge_by_input_id_mut(13).unwrap().behavior = EdgeBehavior::Once;
            runtime_graph.run(&graph);
            assert_eq!(runtime_graph.nodes.iter().all(|_node| _node.has_outputs), true);
            assert_eq!(runtime_graph.node_by_id(1).unwrap().should_execute, false);
            assert_eq!(runtime_graph.node_by_id(2).unwrap().should_execute, false);
            assert_eq!(runtime_graph.node_by_id(3).unwrap().should_execute, false);
            assert_eq!(runtime_graph.node_by_id(4).unwrap().should_execute, false);
            assert_eq!(runtime_graph.node_by_id(5).unwrap().should_execute, true);
        }
        {
            graph.edge_by_input_id_mut(11).unwrap().behavior = EdgeBehavior::Always;
            runtime_graph.run(&graph);
            assert_eq!(runtime_graph.nodes.iter().all(|_node| _node.has_outputs), true);
            assert_eq!(runtime_graph.node_by_id(1).unwrap().should_execute, false);
            assert_eq!(runtime_graph.node_by_id(2).unwrap().should_execute, true);
            assert_eq!(runtime_graph.node_by_id(3).unwrap().should_execute, true);
            assert_eq!(runtime_graph.node_by_id(4).unwrap().should_execute, true);
            assert_eq!(runtime_graph.node_by_id(5).unwrap().should_execute, true);
        }
    }
}
