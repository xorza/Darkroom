//! One frame's photometric normalization against the set's reference, `global` as the light preset
//! takes it, between two lights as the warp leaves them, the second missing the dither's margin:
//! the pixels both cover, their medians and the paired fit of the gain. The flat preset's
//! `multiplicative` over frames that cover every pixel divides the medians their statistics
//! already hold, and has no work of its own to measure.

use common::CancelToken;
use criterion::{BenchmarkId, Criterion, Throughput};
use imaginarium::Buffer2;

use crate::bench::fixture::{Kind, Scale};
use crate::bench::scene::{FRAME, Field};
use crate::combine::frame_cache::slots::Slots;
use crate::combine::normalization::FrameNorm;
use crate::combine::stack_config::Normalization;
use crate::frame::frame_quality::FrameQuality;
use crate::frame::frame_stats::FrameStats;
use crate::frame_store::stored_frame::StoredFrame;
use crate::image::image_dimensions::ImageDimensions;

/// The dither between the two lights, in pixels: the margin the second leaves uncovered.
const DITHER: [usize; 2] = [11, 7];

pub(crate) fn atomic(c: &mut Criterion) {
    let mut group = Kind::Atomic.group(c, "combine/normalization/frame", Scale::Frame);
    group.throughput(Throughput::Elements(FRAME.pixel_count() as u64));
    let field = Field::Typical;
    group.bench_function(BenchmarkId::new("global", field.name()), |b| {
        let frames: Vec<StoredFrame> = (0..2)
            .map(|index| {
                let image = field.aligned(FRAME, index).image;
                let coverage = Buffer2::new(
                    FRAME.width,
                    FRAME.height,
                    (0..FRAME.pixel_count())
                        .map(|pixel| {
                            let (x, y) = (pixel % FRAME.width, pixel / FRAME.width);
                            let outside = x >= FRAME.width - DITHER[0] || y < DITHER[1];
                            if index > 0 && outside { 0.0 } else { 1.0 }
                        })
                        .collect(),
                );
                let quality = FrameQuality::Planes {
                    confidence: coverage.clone(),
                    coverage,
                };
                let stats = FrameStats::measure(&image);
                StoredFrame::from_memory(image, quality, stats)
            })
            .collect();
        let dimensions = ImageDimensions::new(FRAME, 1);
        b.iter(|| {
            FrameNorm::measure(
                &frames,
                dimensions,
                Slots::new(None, 1),
                Normalization::Global,
                &CancelToken::never(),
            )
            .unwrap()
        });
    });
}
