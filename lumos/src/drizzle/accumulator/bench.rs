//! One frame's drops into the drizzle accumulator, as the drizzle adds every frame of a stack: for
//! every kernel the configuration offers, at the pixfrac it is run at, under each geometry that
//! takes its own path through the kernel. The output is twice the frame's side, and Lanczos's
//! the frame's own, the only scale it resamples at.

use criterion::{BenchmarkId, Criterion, Throughput};
use glam::DVec2;

use crate::bench::fixture::{self, Scale};
use crate::bench::scene::{FRAME, Field};
use crate::drizzle::accumulator::DrizzleAccumulator;
use crate::drizzle::deposit::Deposit;
use crate::drizzle::drizzle_config::{DrizzleConfig, DrizzleKernel};
use crate::drizzle::drizzle_frame::DrizzleFrame;
use crate::image::image_dimensions::ImageDimensions;
use crate::registration::bench::Pair;
use crate::registration::transform::Transform;
use crate::registration::transform::warp_transform::WarpTransform;

/// Each kernel and its pixfrac: the default 0.8, and the square kernel at 1 too, where its drops
/// tile the output; Lanczos takes 1 alone.
const KERNELS: [(&str, DrizzleKernel, f32); 6] = [
    ("point_p0.8", DrizzleKernel::Point, 0.8),
    ("turbo_p0.8", DrizzleKernel::Turbo, 0.8),
    ("square_p0.8", DrizzleKernel::Square, 0.8),
    ("square_p1", DrizzleKernel::Square, 1.0),
    ("gaussian_p0.8", DrizzleKernel::Gaussian, 0.8),
    ("lanczos_p1", DrizzleKernel::Lanczos, 1.0),
];

/// How the frame lies on the output: a dither alone; a dither and the turn of field rotation; 45°
/// and 90° turns, where the turbo kernel's axis-aligned drops are worst and exact; and the warp
/// registration fits through a lens, with its SIP correction.
#[derive(Debug, Clone, Copy)]
enum Geometry {
    Turned { name: &'static str, degrees: f64 },
    Sip,
}

const GEOMETRIES: [Geometry; 5] = [
    Geometry::Turned {
        name: "aligned",
        degrees: 0.0,
    },
    Geometry::Turned {
        name: "rotated",
        degrees: 1.0,
    },
    Geometry::Turned {
        name: "45deg",
        degrees: 45.0,
    },
    Geometry::Turned {
        name: "90deg",
        degrees: 90.0,
    },
    Geometry::Sip,
];

impl Geometry {
    const fn name(self) -> &'static str {
        match self {
            Geometry::Turned { name, .. } => name,
            Geometry::Sip => "sip",
        }
    }

    /// The warp from the output's reference frame to the frame: a dither of (0.37, −0.29) px, then
    /// the turn about the frame's centre.
    fn warp(self) -> WarpTransform {
        match self {
            Geometry::Turned { degrees, .. } => {
                let centre = DVec2::new(FRAME.width as f64, FRAME.height as f64) / 2.0;
                let placement = Transform::translation(DVec2::new(0.37, -0.29))
                    .compose(&Transform::rotation_around(centre, degrees.to_radians()));
                WarpTransform::new(placement.inverse())
            }
            Geometry::Sip => Pair::typical().sip_warp(),
        }
    }
}

pub(crate) fn benches(c: &mut Criterion) {
    let field = Field::Typical;
    let mut group = fixture::group(c, "kernel/drizzle/accumulate/frame", Scale::Frame);
    group.throughput(Throughput::Elements(FRAME.pixel_count() as u64));
    for (kernel_name, kernel, pixfrac) in KERNELS {
        for geometry in GEOMETRIES {
            let id = format!("{kernel_name}/{}", geometry.name());
            group.bench_function(BenchmarkId::new(id, field.name()), |b| {
                let config = DrizzleConfig {
                    scale: if kernel == DrizzleKernel::Lanczos {
                        1.0
                    } else {
                        2.0
                    },
                    pixfrac,
                    kernel,
                    ..DrizzleConfig::default()
                };
                let mut accumulator = DrizzleAccumulator::new(
                    ImageDimensions::new(FRAME, 1),
                    Deposit::Channels(1),
                    config,
                    0,
                )
                .expect("a valid drizzle configuration");
                let frame = DrizzleFrame::new(field.render(FRAME).image, geometry.warp());
                b.iter(|| accumulator.add_frame(&frame).expect("the frame drizzles"));
            });
        }
    }
}
