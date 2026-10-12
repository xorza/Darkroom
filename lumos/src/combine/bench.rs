//! A set of 30 aligned lights combined into the stacked product with its standard quality planes,
//! as `stack_stored_frames` combines the frames a run stores: under the light preset with the
//! frames resident, and spilled on the memory of an 8 GiB machine, which a 30-frame 24 MP set
//! does not fit; and under every rejection the configuration offers, resident.

use std::cell::OnceCell;

use common::{CancelToken, TempDir};
use criterion::{BatchSize, BenchmarkId, Criterion, Throughput};

use crate::bench::fixture::{self, Scale};
use crate::bench::scene::{FRAME, Field};
use crate::combine::config::{Combine, CombineMethod, StackConfig};
use crate::combine::rejection::Rejection;
use crate::combine::rejection::linear_fit_clip_config::LinearFitClipConfig;
use crate::combine::rejection::trim_config::TrimConfig;
use crate::combine::stack::stack_stored_frames;
use crate::frame_store::frame_quality::FrameQuality;
use crate::frame_store::frame_stats::FrameStats;
use crate::frame_store::run_scratch::RunScratch;
use crate::frame_store::stored_frame::StoredFrame;
use crate::frame_store::tier::{CombineBudget, Tier};
use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::linear::LinearImage;
use crate::memory::run_memory::RunMemory;
use crate::progress::progress_callback::ProgressCallback;
use crate::stack_product::StackProduct;

/// The lights of the light preset's typical stack.
const LIGHTS: usize = 30;

/// The machine the spilled combine plans against.
const SPILLED_MACHINE_BYTES: u64 = 8 << 30;

pub(crate) fn benches(c: &mut Criterion) {
    let field = Field::Typical;
    let lights = OnceCell::new();
    let lights = || lights.get_or_init(|| Lights::of(field));
    let set = format!("{LIGHTS}x{}", field.name());
    let mut group = fixture::group(c, "stage/combine", Scale::Frame);
    group.throughput(Throughput::Elements((LIGHTS * FRAME.pixel_count()) as u64));
    group.bench_function(BenchmarkId::new("light_preset/resident", &set), |b| {
        let lights = lights();
        b.iter_batched(
            || lights.resident(),
            |frames| lights.combine(frames, Tier::Resident, &StackConfig::light()),
            BatchSize::PerIteration,
        );
    });
    group.bench_function(BenchmarkId::new("light_preset/spilled", &set), |b| {
        let lights = lights();
        let dir = TempDir::new("lumos-bench-combine");
        b.iter_batched(
            || lights.spilled(&dir),
            |Spilled { frames, tier }| lights.combine(frames, tier, &StackConfig::light()),
            BatchSize::PerIteration,
        );
    });
    for (name, rejection) in [
        ("none", Rejection::None),
        ("sigma_clip", Rejection::sigma_clip(2.5)),
        ("winsorized", Rejection::winsorized(3.0)),
        (
            "linear_fit",
            Rejection::LinearFit(LinearFitClipConfig::default()),
        ),
        ("trim", Rejection::Trim(TrimConfig::default())),
        ("gesd", Rejection::gesd()),
    ] {
        let config = StackConfig {
            combine: Combine {
                method: CombineMethod::Mean(rejection),
                ..Combine::mean()
            },
            ..StackConfig::light()
        };
        group.bench_function(BenchmarkId::new(format!("{name}/resident"), &set), |b| {
            let lights = lights();
            b.iter_batched(
                || lights.resident(),
                |frames| lights.combine(frames, Tier::Resident, &config),
                BatchSize::PerIteration,
            );
        });
    }
}

/// The aligned lights of a sequence on a field, and each one's statistics.
#[derive(Debug)]
struct Lights {
    images: Vec<LinearImage>,
    stats: Vec<FrameStats>,
}

impl Lights {
    fn of(field: Field) -> Self {
        let images: Vec<LinearImage> = (0..LIGHTS)
            .map(|index| field.aligned(FRAME, index).image)
            .collect();
        let stats = images.iter().map(FrameStats::measure).collect();
        Self { images, stats }
    }

    /// Every light stored in memory, a copy the combine consumes.
    fn resident(&self) -> Vec<StoredFrame> {
        self.images
            .iter()
            .zip(&self.stats)
            .map(|(image, stats)| {
                StoredFrame::from_memory(image.clone(), FrameQuality::None, stats.clone())
            })
            .collect()
    }

    /// Every light written to a fresh scratch under `dir`, and the spilled tier that holds it.
    fn spilled(&self, dir: &TempDir) -> Spilled {
        let scratch = RunScratch::create(dir.path()).unwrap();
        let frames = self
            .images
            .iter()
            .zip(&self.stats)
            .map(|(image, stats)| {
                StoredFrame::spill(&scratch, image, &FrameQuality::None, stats.clone()).unwrap()
            })
            .collect();
        let budget =
            RunMemory::new(SPILLED_MACHINE_BYTES, None).budget(rayon::current_num_threads());
        let tier = Tier::Spilled {
            scratch,
            combine: CombineBudget::new(budget),
        };
        Spilled { frames, tier }
    }

    fn combine(&self, frames: Vec<StoredFrame>, tier: Tier, config: &StackConfig) -> StackProduct {
        stack_stored_frames(
            frames,
            tier,
            ImageDimensions::new(FRAME, 1),
            self.images[0].metadata.clone(),
            config,
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .unwrap()
    }
}

/// Lights written to the scratch the spilled tier holds.
#[derive(Debug)]
struct Spilled {
    frames: Vec<StoredFrame>,
    tier: Tier,
}
