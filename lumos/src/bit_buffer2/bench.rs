//! The dilation of a frame's source mask, as background refinement grows the sources before it
//! measures the sky around them, by the radius the crowded-field preset dilates with.

use criterion::{BenchmarkId, Criterion, Throughput};

use crate::bench::fixture::{self, Kind, Scale};
use crate::bench::scene::{FRAME, Field};
use crate::bit_buffer2::BitBuffer2;
use crate::star_detection::bench;
use crate::star_detection::bench::Stages;
use crate::star_detection::star_detection_config::background_config::{
    BackgroundRefinement, IterativeRefinement,
};

pub(crate) fn atomic(c: &mut Criterion) {
    let field = Field::Typical;
    let mut group = Kind::Atomic.group(c, "detection/dilation/frame", Scale::Frame);
    group.throughput(Throughput::Elements(FRAME.pixel_count() as u64));
    group.bench_function(BenchmarkId::new("mask", field.name()), |b| {
        let BackgroundRefinement::Iterative(IterativeRefinement { mask_dilation, .. }) =
            bench::config(Field::Dense).background.refinement
        else {
            unreachable!("the crowded-field preset refines the background")
        };
        let mask = Stages::of(field).mask;
        let mut scratch = BitBuffer2::new_default(FRAME);
        b.iter_batched_ref(
            || mask.clone(),
            |mask| mask.dilate(mask_dilation, &mut scratch),
            fixture::fresh_copies(size_of_val(mask.words.as_slice())),
        );
    });
}
