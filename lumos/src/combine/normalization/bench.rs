//! One frame's photometric normalization against the set's reference, `global` as the light preset
//! takes it, between two lights as the warp leaves them, the second missing the dither's margin:
//! the pixels both cover, their medians and the paired fit of the gain. The flat preset's
//! `multiplicative` over frames that cover every pixel divides the medians their statistics
//! already hold, and has no work of its own to measure.

use common::{Buffer2, CancelToken};
use criterion::{BenchmarkId, Criterion, Throughput};

use crate::bench::fixture::{Kind, Scale};
use crate::bench::scene::{FRAME, Field};
use crate::combine::frame_read::slots::Slots;
use crate::combine::normalization::slot_planes::SlotPlanes;
use crate::combine::normalization::{FrameNorm, Normalization};
use crate::frame::admitted::Admitted;
use crate::frame::frame_quality::FrameQuality;
use crate::frame_store::stored_frame::StoredFrame;
use crate::image::channel_layout::ChannelLayout;
use crate::image::image_dimensions::ImageDimensions;

/// The dither between the two lights, in pixels: the margin the second leaves uncovered.
const DITHER: [usize; 2] = [11, 7];

pub(crate) fn atomic(c: &mut Criterion) {
    let mut group = Kind::Atomic.group(c, "combine/normalization/frame", Scale::Frame);
    group.throughput(Throughput::Elements(FRAME.pixel_count() as u64));
    let field = Field::Typical;
    group.bench_function(BenchmarkId::new("global", field.name()), |b| {
        let frames: Vec<StoredFrame> = (0..2)
            .map(|idx| {
                let image = field.aligned(FRAME, idx).image;
                let coverage = Buffer2::from_fn(FRAME.width, FRAME.height, |x, y| {
                    let outside = x >= FRAME.width - DITHER[0] || y < DITHER[1];
                    if idx > 0 && outside { 0.0 } else { 1.0 }
                });
                let quality = FrameQuality::Planes {
                    confidence: coverage.clone(),
                    coverage,
                };
                let stats = Admitted::measure(&image);
                StoredFrame::from_memory(image, quality, stats)
            })
            .collect();
        let dimensions = ImageDimensions::new(FRAME, ChannelLayout::Mono);
        b.iter(|| {
            FrameNorm::measure(
                SlotPlanes {
                    frames: &frames,
                    dimensions,
                    slots: Slots::new(None, 1),
                    cancel: &CancelToken::never(),
                },
                Normalization::Global,
            )
            .unwrap()
        });
    });
}
