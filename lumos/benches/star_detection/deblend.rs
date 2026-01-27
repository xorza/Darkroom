use criterion::{criterion_group, criterion_main};

fn deblend_benchmarks(c: &mut criterion::Criterion) {
    lumos::bench::deblend::benchmarks(c);
}

criterion_group!(benches, deblend_benchmarks);
criterion_main!(benches);
