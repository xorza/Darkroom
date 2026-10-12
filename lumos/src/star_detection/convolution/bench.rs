//! The matched filter over a whole frame's residual, in place, at the FWHM the frame's stars give:
//! circular, which filters in a row pass and a column pass, and elongated at an angle off the
//! pixel axes, as tracking error leaves stars, which takes the full 2D kernel.

use std::cell::OnceCell;

use criterion::{BenchmarkId, Criterion, Throughput};
use imaginarium::Buffer2;

use crate::bench::fixture::{self, Scale};
use crate::bench::scene::{FRAME, Field};
use crate::star_detection::bench::Stages;
use crate::star_detection::convolution::{FilterKernels, matched_filter};
use crate::star_detection::star_detection_config::fwhm_config::MatchedFilter;

pub(crate) fn benches(c: &mut Criterion) {
    let field = Field::Typical;
    let stages = OnceCell::new();
    let mut group = fixture::group(c, "kernel/detection/convolution/frame", Scale::Frame);
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
            let residual = &stages.frame.measure;
            let mut temp = Buffer2::new_default(FRAME.width, FRAME.height);
            let mut kernels = FilterKernels::default();
            b.iter_batched_ref(
                || residual.clone(),
                |values| matched_filter(values, filter, &mut temp, &mut kernels),
                fixture::fresh_copies(size_of_val(residual.pixels())),
            );
        });
    }
}
