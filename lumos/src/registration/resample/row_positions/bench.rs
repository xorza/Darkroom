//! The source position of every pixel of one output row, which the warp evaluates once a row for
//! every reader of it, a row a task on the global pool: under a similarity, which takes the
//! affine path, under a homography, and under a similarity with a SIP correction.

use std::cell::OnceCell;

use criterion::{BenchmarkId, Criterion, Throughput};

use crate::bench::fixture::{self, Scale};
use crate::bench::scene::FRAME;
use crate::registration::bench::Pair;
use crate::registration::final_fit::FitModel;
use crate::registration::resample::row_positions::RowPositions;
use crate::registration::transform::TransformType;

pub(crate) fn benches(c: &mut Criterion) {
    let pair = OnceCell::new();
    let mut group = fixture::group(c, "kernel/registration/row_positions", Scale::Sample);
    group.throughput(Throughput::Elements(FRAME.width as u64));
    for (name, transform) in [
        ("affine", Some(TransformType::Similarity)),
        ("homography", Some(TransformType::Homography)),
        ("sip", None),
    ] {
        group.bench_function(BenchmarkId::new(name, FRAME.width), |b| {
            let pair: &Pair = pair.get_or_init(Pair::typical);
            let warp = transform.map_or_else(
                || pair.sip_warp(),
                |transform| {
                    pair.fit(FitModel {
                        transform,
                        sip: None,
                    })
                    .warp
                },
            );
            let mut positions = RowPositions::default();
            b.iter(|| positions.fill(FRAME.height / 2, FRAME.width, &warp, FRAME));
        });
    }
}
