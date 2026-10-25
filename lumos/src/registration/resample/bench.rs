//! A light warped onto the reference, as the warp stage warps every light into the buffers it
//! keeps from frame to frame, with the coverage and confidence the combine reads: a mono light
//! under every interpolation method, and a demosaiced one under the default Lanczos-3, through the
//! similarity registration fits and through the same with its SIP correction; and a mono light
//! whose calibration flagged its star cores and its defects, under Lanczos-3.

use std::cell::OnceCell;

use common::bench::{Kind, Scale};
use criterion::{Bencher, BenchmarkId, Criterion, Throughput};

use crate::bench::scene::{FRAME, Field};
use crate::image::cfa::cfa_pattern::CfaPattern;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::cfa::demosaic::xtrans::markesteijn::MarkesteijnPasses;
use crate::image::cfa::mosaic::Mosaic;
use crate::image::flags::pixel_flags::PixelFlags;
use crate::image::flags::quality_flags::QualityFlags;
use crate::image::linear_image::LinearImage;
use crate::registration::bench::Pair;
use crate::registration::resample::source_image::SourceImage;
use crate::registration::resample::warp_config::{InterpolationMethod, WarpConfig};
use crate::registration::resample::{Warp, WarpResult, WarpScratch};
use crate::registration::warp_transform::WarpTransform;

pub(crate) fn integration(c: &mut Criterion) {
    let field = Field::Typical;
    let pair = OnceCell::new();
    let similarity = || pair.get_or_init(Pair::typical).similarity_warp();
    let mut group = Kind::Integration.group(c, "warp", Scale::Millis);
    group.throughput(Throughput::Elements(FRAME.pixel_count() as u64));
    for method in InterpolationMethod::ALL {
        let id = format!("{}/mono", name(method));
        group.bench_function(BenchmarkId::new(id, field.name()), |b| {
            let light = field.render(FRAME).image;
            warp(
                b,
                WarpCase {
                    light: &light,
                    transform: &similarity(),
                    method,
                },
            );
        });
    }
    let rgb = || {
        field
            .mosaic(FRAME, CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Rggb)))
            .demosaic_measured(MarkesteijnPasses::One)
    };
    let method = InterpolationMethod::default();
    let id = |variant: &str| BenchmarkId::new(format!("{}/{variant}", name(method)), field.name());
    group.bench_function(id("rgb"), |b| {
        warp(
            b,
            WarpCase {
                light: &rgb(),
                transform: &similarity(),
                method,
            },
        );
    });
    group.bench_function(id("mono_flagged"), |b| {
        let mut light = field.render(FRAME).image;
        light.set_flags(calibration_flags(&light));
        warp(
            b,
            WarpCase {
                light: &light,
                transform: &similarity(),
                method,
            },
        );
    });
    group.bench_function(id("rgb_sip"), |b| {
        warp(
            b,
            WarpCase {
                light: &rgb(),
                transform: &pair.get_or_init(Pair::typical).sip_warp(),
                method,
            },
        );
    });
}

/// The flags a calibration gives `light`: its star cores saturated, and one pixel in
/// [`DEFECT_SPACING`] a defect, scattered by a multiplicative hash of its index.
fn calibration_flags(light: &LinearImage) -> Option<PixelFlags> {
    let pixels = light.channel(0).pixels();
    PixelFlags::from_fn(light.desc().size(), |idx| {
        if (idx as u64)
            .wrapping_mul(0x9e37_79b9_7f4a_7c15)
            .is_multiple_of(DEFECT_SPACING)
        {
            QualityFlags::DEFECT
        } else if pixels[idx] > SATURATION {
            QualityFlags::SATURATED
        } else {
            QualityFlags::default()
        }
    })
}

/// A sensor's defect rate, about 0.1%, as a hot-pixel map flags it.
const DEFECT_SPACING: u64 = 1000;

/// Where the scene's brightest stars clip.
const SATURATION: f32 = 0.8;

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

/// `light` warped by `transform`, sampled by `method`.
#[derive(Debug, Clone, Copy)]
struct WarpCase<'a> {
    light: &'a LinearImage,
    transform: &'a WarpTransform,
    method: InterpolationMethod,
}

fn warp(b: &mut Bencher<'_>, case: WarpCase<'_>) {
    let WarpCase {
        light,
        transform,
        method,
    } = case;
    let warp = Warp {
        transform,
        params: WarpConfig {
            method,
            ..WarpConfig::default()
        },
    };
    let source = SourceImage::of(light);
    let mut buffers = WarpResult::new(light.desc());
    let scratch = WarpScratch::default();
    b.iter(|| buffers.warp_into(&source, warp, &scratch));
}
