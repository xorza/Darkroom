use criterion::{criterion_group, criterion_main};

fn median_filter_benchmarks(c: &mut criterion::Criterion) {
    lumos::bench::median_filter::bench_median_filter(c);
}

criterion_group!(benches, median_filter_benchmarks);
criterion_main!(benches);
