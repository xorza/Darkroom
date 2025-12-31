use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion};

use graph::{
    graph::test_graph,
    prelude::{test_func_lib, ExecutionGraph, TestFuncHooks},
};

fn bench_foo(c: &mut Criterion) {
    c.bench_function("foo", |b| {
        let graph = test_graph();
        let func_lib = test_func_lib(TestFuncHooks::default());

        b.iter(|| {
            let mut execution_graph = ExecutionGraph::default();
            execution_graph
                .update(&graph, &func_lib)
                .expect("Failed to update execution graph for benchmark");
            black_box(execution_graph);
        })
    });
}

criterion_group!(benches, bench_foo);
criterion_main!(benches);
