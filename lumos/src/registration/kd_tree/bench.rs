//! The kd-tree the final fit finds each predicted star's nearest target in: its build over a
//! typical field's catalogue, and a nearest query for every reference star at the position the
//! fitted transform predicts.

use std::cell::OnceCell;
use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput};
use glam::DVec2;

use crate::bench::fixture::{self, Kind, Scale};
use crate::bench::scene::Field;
use crate::registration::bench::Pair;
use crate::registration::kd_tree::KdTree;

pub(crate) fn atomic(c: &mut Criterion) {
    let pair = OnceCell::new();
    let pair = || pair.get_or_init(Pair::typical);
    let count = Field::Typical.stars();
    let mut group = Kind::Atomic.group(c, "registration/kd_tree", Scale::Sample);
    group.throughput(Throughput::Elements(count as u64));
    group.bench_function(BenchmarkId::new("kd_tree/build", count), |b| {
        let points: Vec<DVec2> = pair().target.iter().map(|star| star.pos).collect();
        b.iter_batched(
            || points.clone(),
            KdTree::build,
            fixture::fresh_copies(size_of_val(points.as_slice())),
        );
    });
    group.bench_function(BenchmarkId::new("kd_tree/nearest", count), |b| {
        let pair = pair();
        let tree = KdTree::build(pair.target.iter().map(|star| star.pos).collect())
            .expect("a catalogue of stars");
        let warp = pair.similarity_warp();
        let queries: Vec<DVec2> = pair.reference.iter().map(|s| warp.apply(s.pos)).collect();
        b.iter(|| {
            for &query in &queries {
                black_box(tree.nearest_one(query));
            }
        });
    });
}
