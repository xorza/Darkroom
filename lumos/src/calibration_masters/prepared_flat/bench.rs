//! A session's flat made into the divisor calibration applies: normalized per colour and floored.

use criterion::{BenchmarkId, Criterion, Throughput};

use crate::bench::fixture::{self, Scale};
use crate::bench::scene::{self, FRAME};
use crate::calibration_masters::calibration_set::CalibrationSet;
use crate::calibration_masters::master_role::MasterRole;
use crate::calibration_masters::prepared_flat::PreparedFlat;
use crate::calibration_masters::subtract_master;
use crate::io::image::cfa::CfaType;
use crate::io::image::cfa::cfa_pattern::CfaPattern;

pub(crate) fn benches(c: &mut Criterion) {
    let cfa_type = CfaType::Bayer(CfaPattern::Rggb);
    let mut group = fixture::group(c, "kernel/calibration/prepared_flat/frame", Scale::Frame);
    group.throughput(Throughput::Elements(FRAME.pixel_count() as u64));
    // The flat with its bias taken out, as `CalibrationMasters::from_images` prepares it.
    group.bench_function(BenchmarkId::new("prepare/rggb", "masters"), |b| {
        let CalibrationSet { bias, flat, .. } = scene::masters(FRAME, cfa_type);
        let (bias, mut flat) = (bias.unwrap(), flat.unwrap());
        subtract_master(&mut flat, &bias, MasterRole::Bias, None).unwrap();
        b.iter_batched(
            || flat.clone(),
            |flat| PreparedFlat::new(flat).unwrap(),
            fixture::fresh_copies(size_of_val(flat.data.pixels())),
        );
    });
}
