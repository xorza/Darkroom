//! A session's flat made into the divisor calibration applies: normalized per color and floored.

use common::bench::{self, Kind, Scale};
use criterion::{BenchmarkId, Criterion, Throughput};

use crate::bench::scene::{self, FRAME};
use crate::calibration::calibration_set::CalibrationSet;
use crate::calibration::prepared_flat::PreparedFlat;
use crate::calibration::subtraction::internals::subtract_bias;
use crate::image::cfa::bayer_pattern::BayerPattern;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::cfa::mosaic::Mosaic;

pub(crate) fn atomic(c: &mut Criterion) {
    let cfa_type = CfaType::Mosaic(Mosaic::Bayer(BayerPattern::Rggb));
    let mut group = Kind::Atomic.group(c, "calibration/prepared_flat/frame", Scale::Millis);
    group.throughput(Throughput::Elements(FRAME.pixel_count() as u64));
    // The flat with its bias taken out, as `CalibrationMasters::from_images` prepares it.
    group.bench_function(BenchmarkId::new("prepare/rggb", "masters"), |b| {
        let CalibrationSet { bias, flat, .. } = scene::masters(FRAME, cfa_type);
        let (bias, mut flat) = (bias.unwrap(), flat.unwrap());
        subtract_bias(&mut flat, &bias);
        b.iter_batched(
            || flat.clone(),
            |flat| PreparedFlat::new(flat).unwrap(),
            bench::fresh_copies(size_of_val(flat.data().pixels())),
        );
    });
}
