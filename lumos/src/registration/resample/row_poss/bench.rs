//! The source position of every pixel of one output row, which the warp evaluates once a row for
//! every reader of it, a row a task on the global pool: under a similarity, which takes the
//! affine path, under a homography, and under a similarity with a SIP correction.

use std::cell::OnceCell;

use common::bench::{Kind, Scale};
use criterion::{BenchmarkId, Criterion, Throughput};

use crate::bench::scene::FRAME;
use crate::registration::bench::{Correction, Pair};
use crate::registration::resample::row_poss::RowPoss;
use crate::registration::transform::TransformType;

pub(crate) fn atomic(c: &mut Criterion) {
    let pair = OnceCell::new();
    let mut group = Kind::Atomic.group(c, "registration/row_poss", Scale::Micros);
    group.throughput(Throughput::Elements(FRAME.width as u64));
    for (name, transform, correction) in [
        ("affine", TransformType::Similarity, Correction::Linear),
        ("homography", TransformType::Homography, Correction::Linear),
        ("sip", TransformType::Similarity, Correction::Sip),
    ] {
        group.bench_function(BenchmarkId::new(name, FRAME.width), |b| {
            let pair: &Pair = pair.get_or_init(Pair::typical);
            let warp = pair.fit(pair.model(transform, correction)).warp;
            let mut poss = RowPoss::default();
            b.iter(|| poss.fill(FRAME.height / 2, &warp, FRAME));
        });
    }
}
