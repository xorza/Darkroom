//! The dilation of a frame's source mask, as background refinement grows the sources before it
//! measures the sky around them: by the radius the crowded-field preset dilates with, and by the
//! widest a configuration takes.

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
    let BackgroundRefinement::Iterative(IterativeRefinement { mask_dilation, .. }) =
        bench::config(Field::Dense).background.refinement
    else {
        unreachable!("the crowded-field preset refines the background")
    };
    let mask = Stages::of(field).mask;
    let mut scratch = BitBuffer2::new_default(FRAME);
    for radius in [mask_dilation, IterativeRefinement::MAX_MASK_DILATION] {
        group.bench_function(
            BenchmarkId::new(format!("radius_{radius}"), field.name()),
            |b| {
                b.iter_batched_ref(
                    || mask.clone(),
                    |mask| mask.dilate(radius, &mut scratch),
                    fixture::fresh_copies(BitBuffer2::heap_bytes(mask.size)),
                );
            },
        );
    }
}
