#[cfg(test)]
mod runtime_tests {
    use crate::graph::*;
    use crate::runtime_graph::RuntimeGraph;

    #[test]
    fn simple_run() {
        let graph = Graph::from_json_file("./test_resources/test_graph.json");
        let mut runtime_graph = RuntimeGraph::new();

        runtime_graph.run(&graph);
        assert_eq!(runtime_graph.nodes().iter().all(|_node| _node.should_execute), true);
        assert_eq!(runtime_graph.nodes().iter().all(|_node| _node.has_outputs), false);
    }

    #[test]
    fn double_run() {
        let graph = Graph::from_json_file("./test_resources/test_graph.json");
        let mut runtime_graph = RuntimeGraph::new();

        runtime_graph.run(&graph);
        assert_eq!(runtime_graph.nodes().iter().all(|_node| _node.should_execute), true);
        assert_eq!(runtime_graph.nodes().iter().all(|_node| _node.has_outputs), false);

        runtime_graph.run(&graph);
        assert_eq!(runtime_graph.nodes().iter().all(|_node| _node.has_outputs), true);
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

        graph.node_by_id_mut(2).unwrap().behavior = NodeBehavior::Active;
        runtime_graph.run(&graph);
        assert_eq!(runtime_graph.nodes().iter().all(|_node| _node.has_outputs), true);
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

         graph.node_by_id_mut(4).unwrap()
            .inputs.get_mut(1).unwrap()
            .binding.as_mut().unwrap()
            .behavior = BindingBehavior::Once;
        // graph.input_by_id_mut(13).unwrap().binding.as_mut().unwrap().behavior = BindingBehavior::Once;

        runtime_graph.run(&graph);
        assert_eq!(runtime_graph.nodes().iter().all(|_node| _node.has_outputs), true);
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

        graph.node_by_id_mut(3).unwrap()
            .inputs.get_mut(0).unwrap()
            .binding.as_mut().unwrap()
            .behavior = BindingBehavior::Always;

        runtime_graph.run(&graph);
        assert_eq!(runtime_graph.nodes().iter().all(|_node| _node.has_outputs), true);
        assert_eq!(runtime_graph.node_by_id(1).unwrap().should_execute, true);
        assert_eq!(runtime_graph.node_by_id(2).unwrap().should_execute, false);
        assert_eq!(runtime_graph.node_by_id(3).unwrap().should_execute, true);  //false
        assert_eq!(runtime_graph.node_by_id(4).unwrap().should_execute, true);
        assert_eq!(runtime_graph.node_by_id(5).unwrap().should_execute, true);
    }

    #[test]
    fn multiple_runs_with_various_modifications() {
        let mut graph = Graph::from_json_file("./test_resources/test_graph.json");
        let mut runtime_graph = RuntimeGraph::new();

        {
            runtime_graph.run(&graph);
        }
        {
            runtime_graph.run(&graph);
            assert_eq!(runtime_graph.nodes().iter().all(|_node| _node.has_outputs), true);
            assert_eq!(runtime_graph.node_by_id(1).unwrap().should_execute, false);
            assert_eq!(runtime_graph.node_by_id(2).unwrap().should_execute, false);
            assert_eq!(runtime_graph.node_by_id(3).unwrap().should_execute, false);
            assert_eq!(runtime_graph.node_by_id(4).unwrap().should_execute, false);
            assert_eq!(runtime_graph.node_by_id(5).unwrap().should_execute, true);
        }
        {
            graph.node_by_id_mut(2).unwrap().behavior = NodeBehavior::Active;
            runtime_graph.run(&graph);
            assert_eq!(runtime_graph.nodes().iter().all(|_node| _node.has_outputs), true);
            assert_eq!(runtime_graph.node_by_id(1).unwrap().should_execute, false);
            assert_eq!(runtime_graph.node_by_id(2).unwrap().should_execute, true);
            assert_eq!(runtime_graph.node_by_id(3).unwrap().should_execute, false);
            assert_eq!(runtime_graph.node_by_id(4).unwrap().should_execute, true);
            assert_eq!(runtime_graph.node_by_id(5).unwrap().should_execute, true);
        }
        {
            // graph.input_by_id_mut(13).unwrap().binding.as_mut().unwrap().behavior = BindingBehavior::Once;
            graph.node_by_id_mut(4).unwrap()
                .inputs.get_mut(1).unwrap()
                .binding.as_mut().unwrap()
                .behavior = BindingBehavior::Once;
            runtime_graph.run(&graph);
            assert_eq!(runtime_graph.nodes().iter().all(|_node| _node.has_outputs), true);
            assert_eq!(runtime_graph.node_by_id(1).unwrap().should_execute, false);
            assert_eq!(runtime_graph.node_by_id(2).unwrap().should_execute, false);
            assert_eq!(runtime_graph.node_by_id(3).unwrap().should_execute, false);
            assert_eq!(runtime_graph.node_by_id(4).unwrap().should_execute, false);
            assert_eq!(runtime_graph.node_by_id(5).unwrap().should_execute, true);
        }
        {
            // graph.input_by_id_mut(11).unwrap().binding.as_mut().unwrap().behavior = BindingBehavior::Always;
            graph.node_by_id_mut(3).unwrap()
                .inputs.get_mut(1).unwrap()
                .binding.as_mut().unwrap()
                .behavior = BindingBehavior::Always;
            runtime_graph.run(&graph);
            assert_eq!(runtime_graph.nodes().iter().all(|_node| _node.has_outputs), true);
            assert_eq!(runtime_graph.node_by_id(1).unwrap().should_execute, false);
            assert_eq!(runtime_graph.node_by_id(2).unwrap().should_execute, true);
            assert_eq!(runtime_graph.node_by_id(3).unwrap().should_execute, true);
            assert_eq!(runtime_graph.node_by_id(4).unwrap().should_execute, true);
            assert_eq!(runtime_graph.node_by_id(5).unwrap().should_execute, true);
        }
    }
}
