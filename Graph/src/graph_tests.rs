#[cfg(test)]
mod graph_tests {
    use std::hint::black_box;
    use crate::graph::*;

    #[test]
    fn from_json() {
        let graph = Graph::from_json_file("./test_resources/test_graph.json");
        black_box(graph);
    }
}
