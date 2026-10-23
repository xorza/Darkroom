//! The defect map of a session's masters: hot pixels from the dark, dead ones from the flat, and
//! the sample of one color that the dark's statistics are taken on.

use std::hint::black_box;

use common::CancelToken;
use criterion::{BenchmarkId, Criterion, Throughput};

use crate::bench::fixture::{Kind, Scale};
use crate::bench::scene::{self, FRAME};
use crate::calibration::calibration_set::CalibrationSet;
use crate::calibration::defect_map::sigma_threshold::SigmaThreshold;
use crate::calibration::defect_map::{DefectMap, sampling};
use crate::calibration::subtraction::internals::subtract_bias;
use crate::image::cfa::cfa_pattern::CfaPattern;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::cfa::mosaic::Mosaic;

pub(crate) fn atomic(c: &mut Criterion) {
    let cfa_type = CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Rggb));
    let mut group = Kind::Atomic.group(c, "calibration/defect_map/frame", Scale::Frame);
    group.throughput(Throughput::Elements(FRAME.pixel_count() as u64));
    // As `CalibrationMasters::from_images` builds it: hot pixels from the dark as stacked, dead
    // ones from the flat with the bias taken out.
    group.bench_function(BenchmarkId::new("build/rggb", "masters"), |b| {
        let CalibrationSet {
            bias, dark, flat, ..
        } = scene::masters(FRAME, cfa_type);
        let (bias, dark, mut flat) = (bias.unwrap(), dark.unwrap(), flat.unwrap());
        subtract_bias(&mut flat, &bias);
        b.iter_with_large_drop(|| {
            DefectMap::new(FRAME)
                .detect_hot(&dark, SigmaThreshold::DEFAULT, &CancelToken::never())
                .unwrap()
                .detect_cold(&flat, &CancelToken::never())
                .unwrap()
        });
    });
    let size = format!("{}x{}", FRAME.width, FRAME.height);
    group.bench_function(BenchmarkId::new("color_sample_indices/rggb", size), |b| {
        b.iter_with_large_drop(|| sampling::color_sample_idxs(black_box(FRAME), cfa_type, 0));
    });
}
