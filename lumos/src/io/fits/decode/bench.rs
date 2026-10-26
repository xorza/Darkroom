//! The FITS decode of a frame from its bytes in memory: the conversion to normalized samples, not
//! the disk.

use common::bench::{Kind, Scale};
use criterion::{BenchmarkId, Criterion, Throughput};

use crate::bench::files::{mono_f32, mosaic_u16};
use crate::bench::scene::{FRAME, Field};
use crate::image::cfa::bayer_pattern::BayerPattern;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::cfa::mosaic::Mosaic;
use crate::image::flat_gain::shared_gains::SharedGains;
use crate::internals::fits::open_fits;
use crate::io::load_ctx::LoadCtx;

pub(crate) fn atomic(c: &mut Criterion) {
    let field = Field::Typical;
    let ctx = LoadCtx::default();
    let mut group = Kind::Atomic.group(c, "io/fits/frame", Scale::Millis);
    group.throughput(Throughput::Elements(FRAME.pixel_count() as u64));
    // A mono camera's 16-bit light, a calibrated or stacked frame in 32-bit float, and a color
    // camera's 16-bit light, which loads as a CFA image.
    group.bench_function(BenchmarkId::new("decode/u16", field.name()), |b| {
        let bytes = mosaic_u16(&field.light(FRAME, CfaType::Mono));
        b.iter_with_large_drop(|| {
            open_fits(&bytes, &ctx)
                .unwrap()
                .linear(&ctx, &SharedGains::default())
                .unwrap()
        });
    });
    group.bench_function(BenchmarkId::new("decode/f32", field.name()), |b| {
        let bytes = mono_f32(&field.render(FRAME).image);
        b.iter_with_large_drop(|| {
            open_fits(&bytes, &ctx)
                .unwrap()
                .linear(&ctx, &SharedGains::default())
                .unwrap()
        });
    });
    group.bench_function(BenchmarkId::new("decode/cfa_u16", field.name()), |b| {
        let mosaic = field.light(FRAME, CfaType::Mosaic(Mosaic::Bayer(BayerPattern::Rggb)));
        let bytes = mosaic_u16(&mosaic);
        b.iter_with_large_drop(|| {
            open_fits(&bytes, &ctx)
                .unwrap()
                .cfa(&ctx, &SharedGains::default())
                .unwrap()
        });
    });
}
