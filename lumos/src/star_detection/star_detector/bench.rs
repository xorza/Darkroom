//! A whole frame's stars as `StarDetector::detect` finds and measures them, under the
//! configuration a user picks for each field, with the detector's buffers kept from frame to frame
//! as a run keeps them.

use criterion::{BenchmarkId, Criterion, Throughput};

use crate::bench::fixture::{Kind, Scale};
use crate::bench::scene::{FRAME, Field};
use crate::star_detection::bench;
use crate::star_detection::star_detector::StarDetector;

pub(crate) fn integration(c: &mut Criterion) {
    let mut group = Kind::Integration.group(c, "detect", Scale::Frame);
    group.throughput(Throughput::Elements(FRAME.pixel_count() as u64));
    for field in [Field::Sparse, Field::Typical, Field::Dense] {
        group.bench_function(BenchmarkId::from_parameter(field.name()), |b| {
            let image = field.render(FRAME).image;
            let mut detector =
                StarDetector::from_config(bench::config(field)).expect("a preset is valid");
            b.iter(|| detector.detect(&image));
        });
    }
}
