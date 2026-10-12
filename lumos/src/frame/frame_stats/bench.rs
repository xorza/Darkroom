//! The statistics every light is measured by as it enters a stack: each slot's level and white
//! noise, on a mono frame by multiresolution support, on a mosaic per colour, and on a mosaic a
//! flat divided, with the share of its noise the flat amplified.

use common::CancelToken;
use criterion::{BenchmarkId, Criterion, Throughput};

use crate::bench::fixture::{self, Scale};
use crate::bench::scene::{self, FRAME, Field};
use crate::calibration::calibration_masters::{CalibrationMasters, DEFAULT_SIGMA_THRESHOLD};
use crate::frame::frame_stats::FrameStats;
use crate::image::cfa::cfa_pattern::CfaPattern;
use crate::image::cfa::cfa_type::CfaType;

pub(crate) fn benches(c: &mut Criterion) {
    let field = Field::Typical;
    let rggb = CfaType::Bayer(CfaPattern::Rggb);
    let mut group = fixture::group(c, "kernel/frame/frame_stats/frame", Scale::Frame);
    group.throughput(Throughput::Elements(FRAME.pixel_count() as u64));
    group.bench_function(BenchmarkId::new("measure/mono", field.name()), |b| {
        let frame = field.render(FRAME);
        b.iter(|| FrameStats::measure(&frame.image));
    });
    group.bench_function(BenchmarkId::new("measure/rggb", field.name()), |b| {
        let mosaic = field.mosaic(FRAME, rggb);
        b.iter(|| FrameStats::measure(&mosaic));
    });
    // A light its masters calibrated carries the flat's gain, which splits its noise.
    group.bench_function(BenchmarkId::new("measure_split/rggb", field.name()), |b| {
        let masters = CalibrationMasters::from_images(
            scene::masters(FRAME, rggb),
            DEFAULT_SIGMA_THRESHOLD,
            &CancelToken::never(),
        )
        .unwrap();
        let mut light = field.light(FRAME, rggb);
        masters.calibrate(&mut light).unwrap();
        b.iter(|| FrameStats::measure(&light));
    });
}
