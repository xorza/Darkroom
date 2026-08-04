use criterion::{criterion_group, criterion_main};
use imaginarium::bench;

criterion_group!(benches, bench::preview);
criterion_main!(benches);
