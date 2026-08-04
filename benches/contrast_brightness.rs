use criterion::{criterion_group, criterion_main};
use imaginarium::bench;

criterion_group!(benches, bench::contrast_brightness);
criterion_main!(benches);
