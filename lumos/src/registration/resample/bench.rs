//! A light warped onto the reference, as the warp stage warps every light into the buffers it
//! keeps from frame to frame, with the coverage and confidence the combine reads: a mono light
//! under every interpolation method, and a demosaiced one under the default Lanczos-3, through the
//! similarity registration fits and through the same with its SIP correction.

use std::cell::OnceCell;

use common::CancelToken;
use criterion::{Bencher, BenchmarkId, Criterion, Throughput};

use crate::bench::fixture::{Kind, Scale};
use crate::bench::scene::{FRAME, Field};
use crate::image::cfa::cfa_pattern::CfaPattern;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::cfa::demosaic::xtrans::markesteijn::MarkesteijnPasses;
use crate::image::linear_image::LinearImage;
use crate::registration::bench::Pair;
use crate::registration::resample::WarpBuffers;
use crate::registration::resample::source_image::SourceImage;
use crate::registration::resample::warp_params::{InterpolationMethod, WarpParams};
use crate::registration::transform::warp_transform::WarpTransform;
use crate::scratch_pool::{Scratch, ScratchPool};

pub(crate) fn integration(c: &mut Criterion) {
    let field = Field::Typical;
    let pair = OnceCell::new();
    let similarity = || pair.get_or_init(Pair::typical).similarity_warp();
    let mut group = Kind::Integration.group(c, "warp", Scale::Frame);
    group.throughput(Throughput::Elements(FRAME.pixel_count() as u64));
    for method in InterpolationMethod::ALL {
        let id = format!("{}/mono", name(method));
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
    let id = |variant: &str| BenchmarkId::new(format!("{}/{variant}", name(method)), field.name());
    group.bench_function(id("rgb"), |b| {
        warp(b, &rgb(), &similarity(), method);
    });
    group.bench_function(id("rgb_sip"), |b| {
        warp(
            b,
            &rgb(),
            &pair.get_or_init(Pair::typical).sip_warp(),
            method,
        );
    });
}

/// The bench id's name for `method`.
const fn name(method: InterpolationMethod) -> &'static str {
    match method {
        InterpolationMethod::Nearest => "nearest",
        InterpolationMethod::Bilinear => "bilinear",
        InterpolationMethod::Bicubic => "bicubic",
        InterpolationMethod::Lanczos2 => "lanczos2",
        InterpolationMethod::Lanczos3 => "lanczos3",
        InterpolationMethod::Lanczos4 => "lanczos4",
    }
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
    let mut buffers = WarpBuffers::build(light.dimensions());
    let rows = ScratchPool::default();
    b.iter(|| buffers.warp_into(&source, transform, params, &rows));
}
