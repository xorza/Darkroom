#[cfg(test)]
mod runtime_tests {
    use std::hint::black_box;
    use crate::graph::*;
    use crate::runtime_graph::RuntimeGraph;

    #[test]
    fn run() {
        let graph = Graph::from_json_file("./test_resources/test_graph.json");

        let mut runtime_graph = RuntimeGraph::new();
        runtime_graph.run(&graph);

        runtime_graph.run(&graph);

        black_box(());
    }
}
