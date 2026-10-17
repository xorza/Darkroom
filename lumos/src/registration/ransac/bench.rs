//! RANSAC on the triangle matches of the default configuration's brightest stars: for a
//! similarity and for a homography, the cheapest and the dearest model `Auto` estimates.

use std::cell::OnceCell;

use criterion::{BenchmarkId, Criterion, Throughput};

use crate::bench::fixture::{Kind, Scale};
use crate::registration::bench::Pair;
use crate::registration::registration_config::RegistrationConfig;
use crate::registration::transform::TransformType;

pub(crate) fn atomic(c: &mut Criterion) {
    let pair = OnceCell::new();
    let config = RegistrationConfig::default();
    let count = config.matching.max_stars;
    let mut group = Kind::Atomic.group(c, "registration/ransac", Scale::Sample);
    group.throughput(Throughput::Elements(count as u64));
    for (name, transform) in [
        ("similarity", TransformType::Similarity),
        ("homography", TransformType::Homography),
    ] {
        group.bench_function(BenchmarkId::new(name, count), |b| {
            let prepared = pair.get_or_init(Pair::typical).prepared(&config);
            let search = prepared.search(&prepared.matches());
            b.iter(|| search.hypothesis(transform).expect("the pair registers"));
        });
    }
}
