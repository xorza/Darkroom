//! A light warped onto the reference, as the warp stage warps every light into the buffers it
//! keeps from frame to frame, with the coverage and confidence the combine reads: a mono light
//! under every interpolation method, and a demosaiced one under the default Lanczos-3, through the
//! similarity registration fits and through the same with its SIP correction.

use std::cell::OnceCell;

use common::CancelToken;
use criterion::{Bencher, BenchmarkId, Criterion, Throughput};

use crate::bench::fixture::{self, Scale};
use crate::bench::scene::{FRAME, Field};
use crate::io::image::cfa::CfaType;
use crate::io::image::cfa::cfa_pattern::CfaPattern;
use crate::io::image::linear::LinearImage;
use crate::io::raw::demosaic::xtrans::markesteijn::MarkesteijnPasses;
use crate::registration::bench::Pair;
use crate::registration::registration_config::{InterpolationMethod, WarpParams};
use crate::registration::resample::WarpBuffers;
use crate::registration::resample::source_image::SourceImage;
use crate::registration::transform::WarpTransform;

pub(crate) fn benches(c: &mut Criterion) {
    let field = Field::Typical;
    let pair = OnceCell::new();
    let similarity = || pair.get_or_init(Pair::typical).similarity_warp();
    let mut group = fixture::group(c, "stage/warp", Scale::Frame);
    group.throughput(Throughput::Elements(FRAME.pixel_count() as u64));
    for method in InterpolationMethod::ALL {
        let id = format!("{}/mono", format!("{method:?}").to_lowercase());
        group.bench_function(BenchmarkId::new(id, field.name()), |b| {
            let light = field.render(FRAME).image;
            warp(b, &light, &similarity(), method);
        });
    }
    let rgb = || {
        field
            .mosaic(FRAME, CfaType::Bayer(CfaPattern::Rggb))
            .demosaic(MarkesteijnPasses::One, &CancelToken::never())
            .expect("never cancelled")
    };
    let method = InterpolationMethod::default();
    group.bench_function(BenchmarkId::new("lanczos3/rgb", field.name()), |b| {
        warp(b, &rgb(), &similarity(), method);
    });
    group.bench_function(BenchmarkId::new("lanczos3/rgb_sip", field.name()), |b| {
        warp(
            b,
            &rgb(),
            &pair.get_or_init(Pair::typical).sip_warp(),
            method,
        );
    });
}

fn warp(
    b: &mut Bencher<'_>,
    light: &LinearImage,
    transform: &WarpTransform,
    method: InterpolationMethod,
) {
    let params = WarpParams {
        method,
        ..WarpParams::default()
    };
    let source = SourceImage::of(light);
    let mut buffers = WarpBuffers::new(light.dimensions());
    b.iter(|| buffers.warp_into(&source, transform, params));
}
