//! The stacked master's image operations, each in place on the image a user applies it to: the
//! stretches, the background neutralization and the denoise on the linear master; the green
//! removal, the background extraction, the HDR compression and the local contrast on the master
//! as a stretch leaves it, neutralized and auto-STF stretched.

use std::cell::OnceCell;

use criterion::{BenchmarkId, Criterion, Throughput};

use crate::bench::fixture::{self, Scale};
use crate::bench::scene::{FRAME, Field};
use crate::io::image::linear::LinearImage;
use crate::{
    ColorMode, Denoise, ExtractBackground, Hdr, LocalContrast, NeutralizeBackground, Scnr, Stretch,
    StretchMethod,
};

pub(crate) fn benches(c: &mut Criterion) {
    let field = Field::Typical;
    let linear = OnceCell::new();
    let linear = || linear.get_or_init(|| field.stacked_rgb(FRAME));
    let stretched = OnceCell::new();
    let stretched = || {
        stretched.get_or_init(|| {
            let mut image = linear().clone();
            Op::Neutralize.apply(&mut image);
            Op::Stf.apply(&mut image);
            image
        })
    };
    let mut group = fixture::group(c, "stage/image_ops", Scale::Frame);
    group.throughput(Throughput::Elements(FRAME.pixel_count() as u64));
    for op in Op::ALL {
        let input = if op.stretched() {
            "stretched_rgb"
        } else {
            "stacked_rgb"
        };
        group.bench_function(BenchmarkId::new(op.name(), input), |b| {
            let image = if op.stretched() {
                stretched()
            } else {
                linear()
            };
            b.iter_batched_ref(
                || image.clone(),
                |image| op.apply(image),
                fixture::in_place(3 * size_of_val(image.channel(0).pixels())),
            );
        });
    }
}

/// An image operation at the settings a user starts from.
#[derive(Debug, Clone, Copy)]
enum Op {
    Stf,
    AutoAsinh,
    /// The generalized hyperbolic stretch at a hyperbolic `b` of 2, pivoting on the sky.
    Ghs,
    Neutralize,
    Denoise,
    Scnr,
    ExtractBackground,
    Hdr,
    LocalContrast,
}

impl Op {
    const ALL: [Self; 9] = [
        Self::Stf,
        Self::AutoAsinh,
        Self::Ghs,
        Self::Neutralize,
        Self::Denoise,
        Self::Scnr,
        Self::ExtractBackground,
        Self::Hdr,
        Self::LocalContrast,
    ];

    const fn name(self) -> &'static str {
        match self {
            Self::Stf => "stf",
            Self::AutoAsinh => "auto_asinh",
            Self::Ghs => "ghs",
            Self::Neutralize => "neutralize",
            Self::Denoise => "denoise",
            Self::Scnr => "scnr",
            Self::ExtractBackground => "extract_background",
            Self::Hdr => "hdr",
            Self::LocalContrast => "local_contrast",
        }
    }

    /// Whether the operation takes the stretched master: the display-domain enhancers do.
    const fn stretched(self) -> bool {
        matches!(
            self,
            Self::Scnr | Self::ExtractBackground | Self::Hdr | Self::LocalContrast
        )
    }

    fn apply(self, image: &mut LinearImage) {
        match self {
            Self::Stf => Stretch::auto_stf().apply(image),
            Self::AutoAsinh => Stretch::auto_asinh().apply(image),
            Self::Ghs => Stretch {
                method: StretchMethod::Ghs {
                    black_point: 0.02,
                    d: 5.0,
                    b: 2.0,
                    sp: 0.05,
                    lp: 0.01,
                    hp: 0.9,
                },
                color: ColorMode::ColorPreserving,
            }
            .apply(image),
            Self::Neutralize => NeutralizeBackground.apply(image),
            Self::Denoise => Denoise::default().apply(image),
            Self::Scnr => Scnr::average_neutral(1.0).apply(image),
            Self::ExtractBackground => ExtractBackground::default().apply(image),
            Self::Hdr => Hdr::default().apply(image),
            Self::LocalContrast => LocalContrast::default().apply(image),
        }
        .expect("the operation applies to an RGB master");
    }
}
