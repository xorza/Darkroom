//! A warped light on the spill tier: written to the run's scratch and mapped back, then read as
//! the combine reads it, copied out of its maps a chunk of rows at a time, from the page cache and,
//! where the system can drop a file's pages, from the disk.

use std::slice;

use common::{Buffer2, MappedFile, TempDir};
use criterion::{BatchSize, BenchmarkId, Criterion, Throughput};

use crate::bench::fixture::{Kind, Scale};
use crate::bench::scene::{FRAME, Field};
use crate::combine::frame_read::frame_chunk::ChunkCopy;
use crate::frame::admitted::Admitted;
use crate::frame::frame_quality::FrameQuality;
use crate::frame_store::run_scratch::RunScratch;
use crate::frame_store::stored_frame::{FrameWrite, SetKind, StoredFrame};
use crate::image::linear_image::LinearImage;

/// The rows of a chunk: what a 30-frame stack of warped mono lights gets on a 1 GiB combine
/// budget, which holds two chunks of every frame's pixel, coverage and confidence, 12 bytes a
/// pixel.
const CHUNK_ROWS: usize = (1 << 30) / (2 * 30 * 12 * FRAME.width);

pub(crate) fn atomic(c: &mut Criterion) {
    let field = Field::Typical;
    let mut group = Kind::Atomic.group(c, "frame_store/spill/frame", Scale::Frame);
    group.throughput(Throughput::Elements(FRAME.pixel_count() as u64));
    // Each write takes new files, whose disk space returns as the frame drops after it.
    group.bench_function(BenchmarkId::new("write", field.name()), |b| {
        let warped = Warped::of(field);
        b.iter_batched(
            || warped.stats.clone(),
            |stats| warped.spill(stats),
            BatchSize::PerIteration,
        );
    });
    // Warm: every page of the frame in the page cache, as it is when the frames fit the memory the
    // system has left. Cold: every page on the disk, as when the frames outgrow it, the frame's
    // pages dropped from the page cache before each pass. One frame takes one of the combine's
    // reader threads.
    for (variant, cold) in [("read/warm", false), ("read/cold", true)] {
        if cold && !MappedFile::EVICTS {
            continue;
        }
        group.bench_function(BenchmarkId::new(variant, field.name()), |b| {
            let warped = Warped::of(field);
            let frame = warped.spill(warped.stats.clone());
            let frames = slice::from_ref(&frame);
            let mut copy = ChunkCopy::new(frames, CHUNK_ROWS * FRAME.width);
            let mut read = || {
                for start in (0..FRAME.height).step_by(CHUNK_ROWS) {
                    let end = (start + CHUNK_ROWS).min(FRAME.height);
                    copy.fill(frames, start * FRAME.width..end * FRAME.width);
                }
            };
            if cold {
                b.iter_batched(
                    || assert!(frame.evict(), "the frame's pages leave the page cache"),
                    |()| read(),
                    BatchSize::PerIteration,
                );
            } else {
                b.iter(read);
            }
        });
    }
}

/// A warped mono light, its quality planes, and the scratch it spills to.
#[derive(Debug)]
struct Warped {
    scratch: RunScratch,
    image: LinearImage,
    quality: FrameQuality<Buffer2<f32>>,
    stats: Admitted,
    _dir: TempDir,
}

impl Warped {
    /// `field` with whole coverage and unit confidence: the values change nothing a write or a read
    /// does.
    fn of(field: Field) -> Self {
        let dir = TempDir::new("lumos-bench-spill");
        let scratch = RunScratch::create_evictable(dir.path()).unwrap();
        let image = field.render(FRAME).image;
        let plane = || Buffer2::new_filled(FRAME.width, FRAME.height, 1.0);
        let quality = FrameQuality::Planes {
            coverage: plane(),
            confidence: plane(),
        };
        let stats = Admitted::measure(&image);
        Self {
            scratch,
            image,
            quality,
            stats,
            _dir: dir,
        }
    }

    fn spill(&self, stats: Admitted) -> StoredFrame {
        FrameWrite {
            image: &self.image,
            quality: self.quality.as_ref(),
            set: SetKind::Unwarped,
        }
        .write(&self.scratch)
        .unwrap()
        .into_frame(stats)
    }
}
