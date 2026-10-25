//! The triangle matching of two catalogues' brightest stars: the 200 the default configuration
//! takes and the 500 the precise wide-field preset takes.

use std::cell::OnceCell;

use common::bench::{Kind, Scale};
use criterion::{BenchmarkId, Criterion, Throughput};

use crate::registration::bench::Pair;
use crate::registration::registration_config::RegistrationConfig;

pub(crate) fn atomic(c: &mut Criterion) {
    let pair = OnceCell::new();
    let mut group = Kind::Atomic.group(c, "registration/triangle", Scale::Micros);
    for config in [
        RegistrationConfig::default(),
        RegistrationConfig::precise_wide_field(),
    ] {
        let count = config.matching.max_stars;
        group.throughput(Throughput::Elements(count as u64));
        group.bench_function(BenchmarkId::new("match", count), |b| {
            let prepared = pair.get_or_init(Pair::typical).prepared(&config);
            b.iter(|| prepared.matches());
        });
    }
}
