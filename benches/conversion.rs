use criterion::{criterion_group, criterion_main};
use imaginarium::bench;

criterion_group!(benches, bench::conversion);
criterion_main!(benches);
