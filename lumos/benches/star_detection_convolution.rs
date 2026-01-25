use criterion::{criterion_group, criterion_main};

fn convolution_benchmarks(c: &mut criterion::Criterion) {
    lumos::bench::convolution::benchmarks(c);
}

criterion_group!(benches, convolution_benchmarks);
criterion_main!(benches);
