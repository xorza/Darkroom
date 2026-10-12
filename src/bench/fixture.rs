//! The policy every case follows: its kind and sampling, the variants an atomic case times, the
//! backends an integration case runs on, and the directory its files go to.

use std::fmt::Debug;
use std::path::PathBuf;
use std::time::Duration;
use std::{env, fs, iter, process, thread};

use criterion::measurement::WallTime;
use criterion::{BatchSize, BenchmarkGroup, Criterion, SamplingMode, Throughput};

use crate::common::color_format::ColorFormat;
#[cfg(feature = "wgpu")]
use std::cell::OnceCell;

#[cfg(feature = "wgpu")]
use crate::gpu::Gpu;
use crate::simd_tier::SimdTier;

/// What a case times, the first part of its id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// One kernel on the unit dispatch hands it.
    Atomic,
    /// A public entry point on the standard frame.
    Integration,
}

impl Kind {
    /// The group `name` of this kind, sampled as `scale` says.
    pub(crate) fn group<'a>(
        self,
        c: &'a mut Criterion,
        name: &str,
        scale: Scale,
    ) -> BenchmarkGroup<'a, WallTime> {
        let kind = match self {
            Self::Atomic => "atomic",
            Self::Integration => "integration",
        };
        let mut group = c.benchmark_group(format!("{kind}/{name}"));
        scale.apply(&mut group);
        group
    }
}

/// How long one iteration of a case takes, which sets how criterion samples it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scale {
    /// A row or a few: microseconds an iteration.
    Sample,
    /// A tile or a frame, up to 300 ms an iteration, so fewer and longer samples.
    Frame,
    /// Past 300 ms an iteration: sampled flat at the fewest samples criterion takes, two
    /// iterations each. `iteration` is the slowest case's as measured, which sets the
    /// measurement time.
    Set { iteration: Duration },
}

impl Scale {
    /// The samples of a [`Scale::Set`] case: the fewest criterion takes.
    const SET_SAMPLES: u32 = 10;

    /// The iterations each sample of a [`Scale::Set`] case takes at the measured iteration.
    /// Criterion's flat sampling warns of a sample of one iteration, which it takes wherever the
    /// measurement time is at most the samples times the iteration: at two, only a case that
    /// runs at twice the measured iteration or slower takes one.
    const SET_ITERATIONS: u32 = 2;

    /// Sets the sampling of the cases `group` registers from here on.
    #[expect(
        clippy::disallowed_methods,
        reason = "`Scale::apply` is the one place a group's sampling is set"
    )]
    pub(crate) fn apply(self, group: &mut BenchmarkGroup<'_, WallTime>) {
        match self {
            Self::Sample => group
                .sample_size(50)
                .sampling_mode(SamplingMode::Auto)
                .warm_up_time(Duration::from_millis(500))
                .measurement_time(Duration::from_secs(1)),
            Self::Frame => group
                .sample_size(10)
                .sampling_mode(SamplingMode::Auto)
                .warm_up_time(Duration::from_secs(1))
                .measurement_time(Duration::from_secs(3)),
            Self::Set { iteration } => group
                .sample_size(Self::SET_SAMPLES as usize)
                .sampling_mode(SamplingMode::Flat)
                .warm_up_time(Duration::from_secs(1))
                .measurement_time(iteration * Self::SET_SAMPLES * Self::SET_ITERATIONS),
        };
    }
}

/// The batch for a routine that changes an input of `bytes` in place, so takes a fresh copy each
/// iteration: as many copies as fit in 128 KiB, at least one. Criterion builds a batch's copies
/// before it times them, so a batch that outgrows the cache times the routine on inputs memory
/// has to bring back.
pub(crate) const fn fresh_copies(bytes: usize) -> BatchSize {
    let copies = (128 * 1024) / if bytes == 0 { 1 } else { bytes };
    BatchSize::NumIterations(if copies == 0 { 1 } else { copies as u64 })
}

/// The throughput of a case over `elements` pixels that reads `element_bytes` bytes for each.
pub(crate) const fn throughput(elements: usize, element_bytes: usize) -> Throughput {
    Throughput::ElementsAndBytes {
        elements: elements as u64,
        bytes: (elements * element_bytes) as u64,
    }
}

/// The id part of `format`: `RGBA u8` is `rgba_u8`.
pub(crate) fn format_id(format: ColorFormat) -> String {
    format.name().to_lowercase().replace(' ', "_")
}

/// The id part of a variant-only enum: `Bilinear` is `bilinear`.
pub(crate) fn variant_id(value: impl Debug) -> String {
    format!("{value:?}").to_lowercase()
}

/// One variant of a kernel an atomic case times: the scalar reference when `kernel` is `None`,
/// else the kernel compiled for one tier this CPU has, which the case may call.
#[derive(Debug)]
pub(crate) struct Variant<K> {
    pub(crate) name: &'static str,
    pub(crate) kernel: Option<K>,
}

impl<K> Variant<K> {
    /// The scalar reference, then the kernel `table` gives each tier this CPU has, narrowest
    /// first: one variant for each kernel dispatch may pick.
    pub(crate) fn all(table: impl Fn(SimdTier) -> Option<K>) -> Vec<Self> {
        let scalar = Self {
            name: "scalar",
            kernel: None,
        };
        let tiers = SimdTier::ALL
            .into_iter()
            .filter(|tier| tier.is_supported())
            .filter_map(|tier| {
                table(tier).map(|kernel| Self {
                    name: tier_id(tier),
                    kernel: Some(kernel),
                })
            });
        iter::once(scalar).chain(tiers).collect()
    }
}

const fn tier_id(tier: SimdTier) -> &'static str {
    match tier {
        #[cfg(target_arch = "x86_64")]
        SimdTier::Sse2 => "sse2",
        #[cfg(target_arch = "x86_64")]
        SimdTier::Ssse3 => "ssse3",
        #[cfg(target_arch = "x86_64")]
        SimdTier::Sse41 => "sse41",
        #[cfg(target_arch = "x86_64")]
        SimdTier::Avx2 => "avx2",
        #[cfg(target_arch = "x86_64")]
        SimdTier::Avx2Fma => "avx2_fma",
        #[cfg(target_arch = "aarch64")]
        SimdTier::Neon => "neon",
    }
}

/// The backends a case may run on: the CPU always, the GPU when this build has `wgpu` and the
/// host an adapter. The device opens in the first GPU case a run selects.
#[derive(Debug)]
pub(crate) struct Backends {
    /// `None` on a host with no adapter; else the device, opened by the first case that asks.
    #[cfg(feature = "wgpu")]
    gpu: Option<OnceCell<Gpu>>,
}

/// One backend a case runs on, the first part of its case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Backend {
    Cpu,
    #[cfg(feature = "wgpu")]
    Gpu,
}

impl Backends {
    /// The backends of this build and host, by listing the adapters: no device opens, so a run
    /// that filters to CPU cases opens none. A host with no adapter says so once on stderr, as
    /// its GPU cases are then missing from the run.
    #[cfg_attr(
        feature = "wgpu",
        expect(
            clippy::print_stderr,
            reason = "the notice is what keeps missing GPU cases from reading as a full run"
        )
    )]
    #[cfg_attr(
        not(feature = "wgpu"),
        expect(
            clippy::missing_const_for_fn,
            reason = "only the adapter listing runs code; the body is one shape in every build"
        )
    )]
    pub(crate) fn probe() -> Self {
        Self {
            #[cfg(feature = "wgpu")]
            gpu: if Gpu::adapter_exists() {
                Some(OnceCell::new())
            } else {
                eprintln!("imaginarium benches: no GPU adapter, so no GPU cases");
                None
            },
        }
    }

    /// Every backend, the CPU first.
    pub(crate) fn all(&self) -> Vec<Backend> {
        #[cfg(feature = "wgpu")]
        if self.gpu.is_some() {
            return vec![Backend::Cpu, Backend::Gpu];
        }
        vec![Backend::Cpu]
    }

    /// The GPU, opened by the first case that asks; [`Self::all`] lists it only with an adapter.
    #[cfg(feature = "wgpu")]
    pub(crate) fn gpu(&self) -> &Gpu {
        self.gpu
            .as_ref()
            .expect("the GPU backend is listed only with an adapter")
            .get_or_init(|| Gpu::new().expect("the adapter the probe listed opens a device"))
    }
}

impl Backend {
    /// The backend's part of a case's id.
    pub(crate) const fn id(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            #[cfg(feature = "wgpu")]
            Self::Gpu => "gpu",
        }
    }

    /// `rest` under this backend: `cpu/avx2`.
    pub(crate) fn case(self, rest: &str) -> String {
        format!("{}/{rest}", self.id())
    }
}

/// A directory for the files the cases write, removed with them when dropped.
#[derive(Debug)]
pub(crate) struct ScratchDir {
    path: PathBuf,
}

impl ScratchDir {
    /// A new directory `name` under the system's temporary directory, unique to this process.
    pub(crate) fn new(name: &str) -> Self {
        let path = env::temp_dir().join(format!("imaginarium-bench-{}-{name}", process::id()));
        fs::create_dir_all(&path).expect("the bench scratch directory is created");
        Self { path }
    }

    /// The path of the file `name` in this directory.
    pub(crate) fn file(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let removed = fs::remove_dir_all(&self.path);
        if !thread::panicking() {
            removed.expect("the bench scratch directory is removed");
        }
    }
}
