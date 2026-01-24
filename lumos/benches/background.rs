use criterion::{criterion_group, criterion_main};

fn background_benchmarks(c: &mut criterion::Criterion) {
    lumos::bench::background::benchmarks(c);
}

criterion_group!(benches, background_benchmarks);
criterion_main!(benches);
