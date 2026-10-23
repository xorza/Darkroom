//! The demosaic of a calibrated light, through `CfaImage::demosaic` as production calls it: the
//! kernels with the camera's white balance applied as their tiles read.

use criterion::{BenchmarkId, Criterion, Throughput};

use crate::bench::fixture::{self, Kind, Scale};
use crate::bench::scene::{FRAME, Field};
use crate::image::cfa::cfa_pattern::CfaPattern;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::cfa::demosaic::xtrans::markesteijn::MarkesteijnPasses;
use crate::image::cfa::mosaic::Mosaic;
use crate::internals::cfa::XTRANS_PATTERN;

pub(crate) fn atomic(c: &mut Criterion) {
    let field = Field::Typical;
    let mut group = Kind::Atomic.group(c, "image/demosaic/frame", Scale::Frame);
    group.throughput(Throughput::Elements(FRAME.pixel_count() as u64));
    // RCD on a Bayer sensor, which takes no pass count, and Markesteijn on an X-Trans one at both
    // the counts a user picks between.
    for (operation, cfa_type, passes) in [
        (
            "rcd/rggb",
            CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Rggb)),
            MarkesteijnPasses::One,
        ),
        (
            "markesteijn/xtrans_1pass",
            CfaType::Mosaic(Mosaic::XTrans(XTRANS_PATTERN)),
            MarkesteijnPasses::One,
        ),
        (
            "markesteijn/xtrans_3pass",
            CfaType::Mosaic(Mosaic::XTrans(XTRANS_PATTERN)),
            MarkesteijnPasses::Three,
        ),
    ] {
        group.bench_function(BenchmarkId::new(operation, field.name()), |b| {
            let mosaic = field.mosaic(FRAME, cfa_type);
            b.iter_batched(
                || mosaic.clone(),
                |mosaic| mosaic.demosaic_measured(passes),
                fixture::fresh_copies(size_of_val(mosaic.data().pixels())),
            );
        });
    }
}
