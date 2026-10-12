//! Every criterion bench of the crate, in one target: `cargo bench -p lumos --features bench`.
//!
//! The drivers live in the library (`lumos::bench`): each sits in a `bench.rs` beside the code it
//! measures and reaches crate privates, which a `benches/*.rs` crate cannot name.

use criterion::{criterion_group, criterion_main};
use lumos::bench;

criterion_group!(kernel, bench::kernel);
criterion_group!(stage, bench::stage);
criterion_main!(kernel, stage);
