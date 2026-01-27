use criterion::{criterion_group, criterion_main};

fn sigma_clipped_benchmarks(c: &mut criterion::Criterion) {
    lumos::bench::sigma_clipped::benchmarks(c);
}

criterion_group!(benches, sigma_clipped_benchmarks);
criterion_main!(benches);
