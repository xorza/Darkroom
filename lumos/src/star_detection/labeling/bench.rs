//! The connected components of a detection plane's threshold mask, with the labeler's buffers kept
//! from frame to frame as the detector keeps them.

use criterion::{BenchmarkId, Criterion, Throughput};

use crate::bench::fixture::{Kind, Scale};
use crate::bench::scene::{FRAME, Field};
use crate::star_detection::bench::Stages;
use crate::star_detection::labeling::labeler::Labeler;

pub(crate) fn atomic(c: &mut Criterion) {
    let mut group = Kind::Atomic.group(c, "detection/labeling/frame", Scale::Frame);
    group.throughput(Throughput::Elements(FRAME.pixel_count() as u64));
    for field in [Field::Typical, Field::Dense] {
        group.bench_function(BenchmarkId::new("label", field.name()), |b| {
            let stages = Stages::of(field);
            let connectivity = stages.config.extraction.connectivity;
            let mut labeler = Labeler::default();
            b.iter(|| labeler.label(&stages.mask, connectivity).label_count());
        });
    }
}
