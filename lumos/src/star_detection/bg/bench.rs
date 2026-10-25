//! The sky and its noise over a whole frame, as the detector's first stage measures them: each
//! tile's clipped statistics, the median filter over the tiles, and the spline between them.

use common::bench::{Kind, Scale};
use criterion::{BenchmarkId, Criterion, Throughput};

use crate::bench::scene::{FRAME, Field};
use crate::image::masked_plane::MaskedPlane;
use crate::star_detection::bench;
use crate::star_detection::bg::bg_estimate::BgEstimate;
use crate::star_detection::resources::DetectionResources;

pub(crate) fn atomic(c: &mut Criterion) {
    let mut group = Kind::Atomic.group(c, "detection/bg/frame", Scale::Millis);
    group.throughput(Throughput::Elements(FRAME.pixel_count() as u64));
    for field in [Field::Typical, Field::Dense] {
        group.bench_function(BenchmarkId::new("mesh", field.name()), |b| {
            let pixels = field.render(FRAME).image.channel(0).clone();
            let config = bench::config(field).bg;
            let mut resources = DetectionResources::new(FRAME);
            b.iter(|| {
                BgEstimate::estimate(
                    MaskedPlane::from(&pixels),
                    &config,
                    &resources.surfaces,
                    &mut resources.bg,
                )
            });
        });
    }
}
