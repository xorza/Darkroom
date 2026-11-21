//! One frame's drops into the drizzle accumulator, as the drizzle adds every frame of a stack: for
//! every kernel the configuration offers, at the pixfrac it is run at, under each geometry that
//! takes its own path through the kernel. The output is twice the frame's side, and Lanczos's
//! the frame's own, the only scale it resamples at.

use common::bench::{Kind, Scale};
use criterion::{BenchmarkId, Criterion, Throughput};
use glam::DVec2;

use crate::bench::scene::{FRAME, Field};
use crate::drizzle::accumulator::Drizzler;
use crate::drizzle::accumulator::kernel_plan::KernelPlan;
use crate::drizzle::drizzle_frame::DrizzleFrame;
use crate::drizzle::drizzle_sampling::{DrizzleSampling, DrizzleScale, DropShape, Pixfrac};
use crate::drizzle::output_grid::OutputGrid;
use crate::registration::bench::Pair;
use crate::registration::transform::Transform;
use crate::registration::warp_transform::WarpTransform;

/// Each sampling: every drop shape at the default pixfrac 0.8, the square at 1 too, where its
/// drops tile, and the point, at scale 2; Lanczos at the frame's own scale, the only one it
/// resamples at.
fn samplings() -> [(&'static str, DrizzleSampling); 6] {
    let scale = DrizzleScale::new(2.0);
    let drops = |shape, pixfrac| DrizzleSampling::Drops {
        shape,
        scale,
        pixfrac: Pixfrac::new(pixfrac),
    };
    [
        ("point_p0.8", DrizzleSampling::Point { scale }),
        ("turbo_p0.8", drops(DropShape::Turbo, 0.8)),
        ("square_p0.8", drops(DropShape::Square, 0.8)),
        ("square_p1", drops(DropShape::Square, 1.0)),
        ("gaussian_p0.8", drops(DropShape::Gaussian, 0.8)),
        ("lanczos_p1", DrizzleSampling::Lanczos),
    ]
}

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
            Self::Turned { name, .. } => name,
            Self::Sip => "sip",
        }
    }

    /// The warp from the output's reference frame to the frame: a dither of (0.37, −0.29) px, then
    /// the turn about the frame's centre.
    fn warp(self) -> WarpTransform {
        match self {
            Self::Turned { degrees, .. } => {
                let centre = DVec2::new(FRAME.width as f64, FRAME.height as f64) / 2.0;
                let placement = Transform::translation(DVec2::new(0.37, -0.29))
                    .compose(&Transform::rotation_around(centre, degrees.to_radians()));
                WarpTransform::from(placement.inverse())
            }
            Self::Sip => Pair::typical().sip_warp(),
        }
    }
}

pub(crate) fn atomic(c: &mut Criterion) {
    let field = Field::Typical;
    let mut group = Kind::Atomic.group(c, "drizzle/accumulate/frame", Scale::Millis);
    group.throughput(Throughput::Elements(FRAME.pixel_count() as u64));
    for (kernel_name, sampling) in samplings() {
        for geometry in GEOMETRIES {
            let id = format!("{kernel_name}/{}", geometry.name());
            group.bench_function(BenchmarkId::new(id, field.name()), |b| {
                let frame = DrizzleFrame::new(field.render(FRAME).image, geometry.warp());
                let mut drizzler = Drizzler::new(
                    &frame.source,
                    KernelPlan::new(sampling),
                    OutputGrid::new(sampling.scale(), FRAME),
                );
                let mut sums = drizzler.sums();
                b.iter(|| {
                    drizzler
                        .scatter(&frame, 0, &mut sums)
                        .expect("the frame drizzles")
                });
            });
        }
    }
}
