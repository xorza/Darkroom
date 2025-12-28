use std::hint::black_box;
use std::path::Path;

use criterion::{criterion_group, criterion_main, Criterion};

use graph::function::test_func_lib;
use graph::graph::Graph;
use graph::runtime_graph::ExecutionGraph;

fn bench_foo(c: &mut Criterion) {
    c.bench_function("foo", |b| {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let graph_path = manifest_dir.join("../test_resources/test_graph.yml");
        let graph = Graph::from_file(
            graph_path
                .to_str()
                .expect("Benchmark graph path is not valid UTF-8"),
        )
        .expect("Failed to load benchmark graph from test_resources");
        let func_lib = test_func_lib();

        b.iter(|| {
            let mut execution_graph = ExecutionGraph::default();
            execution_graph.update(&graph, &func_lib);
            black_box(execution_graph);
        })
    });
}

criterion_group!(benches, bench_foo);
criterion_main!(benches);
