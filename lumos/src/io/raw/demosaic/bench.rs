//! The demosaic of a calibrated light, through `CfaImage::demosaic` as production calls it: the
//! kernels with the camera's white balance applied as their tiles read.

use common::CancelToken;
use criterion::{BenchmarkId, Criterion, Throughput};

use crate::bench::fixture::{self, Scale};
use crate::bench::scene::{FRAME, Field};
use crate::internals::cfa::XTRANS_PATTERN;
use crate::io::image::cfa::CfaType;
use crate::io::image::cfa::cfa_pattern::CfaPattern;
use crate::io::raw::demosaic::xtrans::markesteijn::MarkesteijnPasses;

pub(crate) fn benches(c: &mut Criterion) {
    let field = Field::Typical;
    let mut group = fixture::group(c, "kernel/io/demosaic/frame", Scale::Frame);
    group.throughput(Throughput::Elements(FRAME.pixel_count() as u64));
    // RCD on a Bayer sensor, which takes no pass count, and Markesteijn on an X-Trans one at both
    // the counts a user picks between.
    for (operation, cfa_type, passes) in [
        (
            "rcd/rggb",
            CfaType::Bayer(CfaPattern::Rggb),
            MarkesteijnPasses::One,
        ),
        (
            "markesteijn/xtrans_1pass",
            CfaType::XTrans(XTRANS_PATTERN),
            MarkesteijnPasses::One,
        ),
        (
            "markesteijn/xtrans_3pass",
            CfaType::XTrans(XTRANS_PATTERN),
            MarkesteijnPasses::Three,
        ),
    ] {
        group.bench_function(BenchmarkId::new(operation, field.name()), |b| {
            let mosaic = field.mosaic(FRAME, cfa_type);
            b.iter_batched(
                || mosaic.clone(),
                |mosaic| mosaic.demosaic(passes, &CancelToken::never()).unwrap(),
                fixture::fresh_copies(size_of_val(mosaic.data.pixels())),
            );
        });
    }
}
