//! A set of 30 aligned lights combined into the stacked product with its standard quality planes,
//! as `stack_stored_frames` combines the frames a run stores: under the light preset with the
//! frames resident, and spilled on the memory of an 8 GiB machine, which a 30-frame 24 MP set
//! does not fit, read from the page cache and, where the system can drop a file's pages, from the
//! disk; and under every rejection the configuration offers, resident.

use std::cell::OnceCell;
use std::time::Duration;

use common::{MappedFile, TempDir};
use criterion::{BatchSize, BenchmarkId, Criterion, Throughput};
use imaginarium::Buffer2;

use crate::bench::fixture::{Kind, Scale};
use crate::bench::scene::{FRAME, Field, STACKED_LIGHTS};
use crate::combine::frame_cache::core::CacheCore;
use crate::combine::frame_cache::sample;
use crate::combine::stack_config::StackConfig;
use crate::combine::stack_config::combine::Combine;
use crate::combine::stack_config::combine_method::CombineMethod;
use crate::combine::{normalization, rejection, stack};
use crate::frame::admitted::Admitted;
use crate::frame::frame_quality::FrameQuality;
use crate::frame_store::run_scratch::RunScratch;
use crate::frame_store::storage_tier::{CombineBudget, StorageTier};
use crate::frame_store::stored_frame::{SpillFrame, StoredFrame};
use crate::image::channel_layout::ChannelLayout;
use crate::image::image_dimensions::ImageDimensions;
use crate::image::linear_image::LinearImage;
use crate::memory::run_memory::RunMemory;
use crate::progress::progress_monitor::ProgressMonitor;
use crate::stack_product::StackProduct;

/// The machine the spilled combine plans against.
const SPILLED_MACHINE_BYTES: u64 = 8 << 30;

pub(crate) fn atomic(c: &mut Criterion) {
    rejection::bench::atomic(c);
    sample::bench::atomic(c);
    normalization::bench::atomic(c);
}

pub(crate) fn integration(c: &mut Criterion) {
    set(c);
    stack::bench::integration(c);
}

fn set(c: &mut Criterion) {
    let field = Field::Typical;
    let lights = OnceCell::new();
    let lights = || lights.get_or_init(|| Lights::of(field));
    let set = format!("{STACKED_LIGHTS}x{}", field.name());
    // The slowest bench's iteration, `light_preset/spilled_cold`'s median on a Ryzen 7 6800U,
    // x86-64-v3.
    let iteration = Duration::from_millis(2550);
    let mut group = Kind::Integration.group(c, "combine", Scale::Set { iteration });
    group.throughput(Throughput::Elements(
        (STACKED_LIGHTS * FRAME.pixel_count()) as u64,
    ));
    group.bench_function(BenchmarkId::new("light_preset/resident", &set), |b| {
        let lights = lights();
        b.iter_batched(
            || lights.resident(),
            |frames| lights.combine(frames, StorageTier::Resident, &StackConfig::light()),
            BatchSize::PerIteration,
        );
    });
    for (variant, cold) in [
        ("light_preset/spilled", false),
        ("light_preset/spilled_cold", true),
    ] {
        if cold && !MappedFile::EVICTS {
            continue;
        }
        group.bench_function(BenchmarkId::new(variant, &set), |b| {
            let lights = lights();
            let dir = TempDir::new("lumos-bench-combine");
            b.iter_batched(
                || {
                    let spilled = lights.spilled(&dir);
                    if cold {
                        for frame in &spilled.frames {
                            assert!(frame.evict(), "the light's pages leave the page cache");
                        }
                    }
                    spilled
                },
                |Spilled { frames, tier }| lights.combine(frames, tier, &StackConfig::light()),
                BatchSize::PerIteration,
            );
        });
    }
    for (name, rejection) in rejection::bench::rejections() {
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
                |frames| lights.combine(frames, StorageTier::Resident, &config),
                BatchSize::PerIteration,
            );
        });
    }
}

/// The aligned lights of a sequence on a field, and each one's statistics.
#[derive(Debug)]
struct Lights {
    images: Vec<LinearImage>,
    stats: Vec<Admitted>,
}

impl Lights {
    fn of(field: Field) -> Self {
        let images: Vec<LinearImage> = (0..STACKED_LIGHTS)
            .map(|idx| field.aligned(FRAME, idx).image)
            .collect();
        let stats = images.iter().map(Admitted::measure).collect();
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
        let scratch = RunScratch::create_evictable(dir.path()).unwrap();
        let frames = self
            .images
            .iter()
            .zip(&self.stats)
            .map(|(image, stats)| {
                SpillFrame {
                    image,
                    quality: &FrameQuality::<Buffer2<f32>>::None,
                    admitted: stats.clone(),
                }
                .spill(&scratch)
                .unwrap()
            })
            .collect();
        let budget =
            RunMemory::new(SPILLED_MACHINE_BYTES, None).budget(rayon::current_num_threads());
        let tier = StorageTier::Spilled {
            scratch,
            combine: CombineBudget::new(budget),
        };
        Spilled { frames, tier }
    }

    fn combine(
        &self,
        frames: Vec<StoredFrame>,
        tier: StorageTier,
        config: &StackConfig,
    ) -> StackProduct {
        config
            .stack_stored_frames(
                frames,
                CacheCore {
                    tier,
                    dimensions: ImageDimensions::new(FRAME, ChannelLayout::Mono),
                    metadata: self.images[0].metadata.clone(),
                    monitor: ProgressMonitor::unattended(),
                },
            )
            .unwrap()
    }
}

/// Lights written to the scratch the spilled tier holds.
#[derive(Debug)]
struct Spilled {
    frames: Vec<StoredFrame>,
    tier: StorageTier,
}
