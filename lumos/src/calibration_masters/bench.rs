//! The calibration of one light by a session's masters: the bias, the dark and the flat removed
//! and the defects repaired, as every light of the session is.

use common::CancelToken;
use criterion::{BenchmarkId, Criterion, Throughput};

use crate::bench::fixture::{self, Scale};
use crate::bench::scene::{self, FRAME, Field};
use crate::calibration_masters::{CalibrationMasters, DEFAULT_SIGMA_THRESHOLD};
use crate::io::image::cfa::CfaType;
use crate::io::image::cfa::cfa_pattern::CfaPattern;

pub(crate) fn benches(c: &mut Criterion) {
    let field = Field::Typical;
    let cfa_type = CfaType::Bayer(CfaPattern::Rggb);
    let mut group = fixture::group(c, "kernel/calibration/apply/frame", Scale::Frame);
    group.throughput(Throughput::Elements(FRAME.pixel_count() as u64));
    // Calibration changes the light in place, so each iteration takes a fresh copy of it.
    group.bench_function(BenchmarkId::new("light/rggb", field.name()), |b| {
        let masters = CalibrationMasters::from_images(
            scene::masters(FRAME, cfa_type),
            DEFAULT_SIGMA_THRESHOLD,
            &CancelToken::never(),
        )
        .unwrap();
        let light = field.light(FRAME, cfa_type);
        b.iter_batched_ref(
            || light.clone(),
            |light| masters.calibrate(light).unwrap(),
            fixture::in_place(size_of_val(light.data.pixels())),
        );
    });
}
