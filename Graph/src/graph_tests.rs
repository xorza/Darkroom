#[cfg(test)]
mod graph_tests {
    use std::hint::black_box;
    use crate::graph::*;

    #[test]
    fn from_json() {
        let graph = Graph::from_json_file("./test_resources/test_graph.json");
        black_box(graph);
    }

    #[test]
    fn node_remove_test() {
        let mut graph = Graph::from_json_file("./test_resources/test_graph.json");

        graph.remove_node_by_id(3);

        assert!(graph.node_by_id(3).is_none());
        assert_eq!(graph.nodes().len(), 4);
        assert_eq!(graph.inputs().len(), 3);
        assert_eq!(graph.outputs().len(), 3);

        for input in graph.inputs() {
            if input.connected_output_id != 0 {
                let output = graph.output_by_id(input.connected_output_id).unwrap();
                assert_ne!(output.node_id(), 3);
            }
        }
    }
}
