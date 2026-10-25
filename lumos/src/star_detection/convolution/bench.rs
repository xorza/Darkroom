//! The matched filter over a whole frame's residual, a band of rows at a time into a detection
//! plane, at the FWHM the frame's stars give: circular, which filters in a row pass and a column
//! pass, and elongated at an angle off the pixel axes, as tracking error leaves stars, which takes
//! the full 2D kernel.

use std::cell::OnceCell;

use common::Buffer2;
use criterion::{BenchmarkId, Criterion, Throughput};

use crate::bench::fixture::{Kind, Scale};
use crate::bench::scene::{FRAME, Field};
use crate::scratch_pool::ScratchPool;
use crate::star_detection::bench::Stages;
use crate::star_detection::convolution::filter_kernels::FilterKernels;
use crate::star_detection::detection_filter::{DetectionFilter, FilterSource};
use crate::star_detection::detection_plane::Prefilter;
use crate::star_detection::star_detection_config::fwhm_config::MatchedFilter;

pub(crate) fn atomic(c: &mut Criterion) {
    let field = Field::Typical;
    let stages = OnceCell::new();
    let mut group = Kind::Atomic.group(c, "detection/convolution/frame", Scale::Frame);
    group.throughput(Throughput::Elements(FRAME.pixel_count() as u64));
    for (shape, axis_ratio, angle) in [("circular", 1.0, 0.0), ("elliptical", 0.7, 0.5)] {
        let id = BenchmarkId::new(format!("matched_filter/{shape}"), field.name());
        group.bench_function(id, |b| {
            let stages: &Stages = stages.get_or_init(|| Stages::of(field));
            let filter = MatchedFilter {
                fwhm: stages.fwhm.expect("the default configuration filters"),
                axis_ratio,
                angle,
            };
            let residual: &Buffer2<f32> = &stages.frame.measure;
            let mut output = Buffer2::new_default(FRAME.width, FRAME.height);
            let mut kernels = FilterKernels::default();
            let bands = ScratchPool::default();
            b.iter(|| {
                DetectionFilter {
                    prefilter: Prefilter::None,
                    matched: Some(kernels.matched(filter)),
                }
                .run(FilterSource::Plane(residual), &mut output, &bands);
            });
        });
    }
}
