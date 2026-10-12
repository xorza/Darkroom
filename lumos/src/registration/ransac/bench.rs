//! RANSAC on the triangle matches of the default configuration's brightest stars: for a
//! similarity and for a homography, the cheapest and the dearest model `Auto` estimates.

use std::cell::OnceCell;

use criterion::{BenchmarkId, Criterion, Throughput};

use crate::bench::fixture::{Kind, Scale};
use crate::registration::bench::Pair;
use crate::registration::ransac::RansacEstimator;
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
            let pair: &Pair = pair.get_or_init(Pair::typical);
            let trees = pair.trees(count);
            let matches = pair.matches(&trees, &config);
            let estimator = RansacEstimator::new(config.ransac.clone(), pair.max_sigma());
            let (reference, target) = (trees.reference.points(), trees.target.points());
            b.iter(|| {
                estimator
                    .estimate(&matches, reference, target, transform)
                    .expect("the pair registers")
            });
        });
    }
}
