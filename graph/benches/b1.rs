use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion};

use graph::function::FuncLib;
use graph::graph::Graph;
use graph::runtime_graph::RuntimeGraph;

fn bench_foo(c: &mut Criterion) {
    c.bench_function("foo", |b| {
        let mut graph = Graph::from_yaml_file("../test_resources/test_graph.yml")
            .expect("Failed to load benchmark graph from ../test_resources/test_graph.yml");
        let func_lib = FuncLib::from_yaml_file("../test_resources/test_funcs.yml")
            .expect("Failed to load func lib from ../test_resources/test_funcs.yml");
        graph
            .node_by_name_mut("sum")
            .unwrap_or_else(|| panic!("Node named \"sum\" not found"))
            .cache_outputs = false;

        b.iter(|| {
            let runtime_graph = RuntimeGraph::new(&graph, &func_lib);
            black_box(runtime_graph);
        })
    });
}

criterion_group!(benches, bench_foo);
criterion_main!(benches);
