use std::hint::black_box;
use std::path::Path;

use criterion::{criterion_group, criterion_main, Criterion};

use graph::function::FuncLib;
use graph::graph::Graph;
use graph::runtime_graph::RuntimeGraph;

fn bench_foo(c: &mut Criterion) {
    c.bench_function("foo", |b| {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let graph_path = manifest_dir.join("../test_resources/test_graph.yml");
        let func_lib_path = manifest_dir.join("../test_resources/test_funcs.yml");
        assert!(
            graph_path.is_file(),
            "Benchmark graph file missing at {}",
            graph_path.display()
        );
        assert!(
            func_lib_path.is_file(),
            "Benchmark func lib file missing at {}",
            func_lib_path.display()
        );
        let mut graph = Graph::from_yaml_file(
            graph_path
                .to_str()
                .expect("Benchmark graph path is not valid UTF-8"),
        )
        .expect("Failed to load benchmark graph from test_resources");
        let func_lib = FuncLib::from_yaml_file(
            func_lib_path
                .to_str()
                .expect("Benchmark func lib path is not valid UTF-8"),
        )
        .expect("Failed to load func lib from test_resources");
        graph
            .node_by_name_mut("sum")
            .unwrap_or_else(|| panic!("Node named \"sum\" not found"))
            .cache_outputs = false;

        b.iter(|| {
            let mut runtime_graph = RuntimeGraph::default();
            runtime_graph.update(&graph, &func_lib);
            black_box(runtime_graph);
        })
    });
}

criterion_group!(benches, bench_foo);
criterion_main!(benches);
