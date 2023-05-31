#[cfg(test)]
mod graph_tests {
    use std::hint::black_box;
    use crate::graph::*;

    #[test]
    fn from_json() {
        let graph = Graph::from_json_file("./test_resources/test_graph.json");
        let yaml = graph.to_yaml();
        let graph = Graph::from_yaml(&yaml);
        black_box(graph);
    }

    #[test]
    fn node_remove_test() {
        let mut graph = Graph::from_json_file("./test_resources/test_graph.json");

        graph.remove_node_by_id(3);

        assert!(graph.node_by_id(3).is_none());
        assert_eq!(graph.nodes().len(), 4);

        for input in graph.nodes().iter().flat_map(|node| node.inputs.iter()) {
            if let Some(binding) = input.binding.as_ref() {
                assert_ne!(binding.output_node_id(), 3);
            }
        }
    }
}
