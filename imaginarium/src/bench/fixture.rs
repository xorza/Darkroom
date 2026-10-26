//! What this crate's cases add to the shared policy of `common::bench`: their throughput and id
//! parts, the variants an atomic case times, and the backends an integration case runs on.

#[cfg(feature = "wgpu")]
use std::cell::OnceCell;
use std::fmt::Debug;
use std::iter;

use criterion::Throughput;
use simd::IsaTier;

use crate::color::color_format::ColorFormat;
#[cfg(feature = "wgpu")]
use crate::gpu::Gpu;

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

/// One variant of a vector kernel an atomic case times: the scalar reference, or the kernel on one
/// tier.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Variant<K> {
    Scalar,
    Kernel { kernel: K, tier: IsaTier },
}

impl<K: Copy> Variant<K> {
    /// The scalar reference, then `kernel` on each tier this CPU has: a case with no kernel times
    /// the reference alone.
    pub(crate) fn all(kernel: Option<K>) -> impl Iterator<Item = Self> {
        let kernels = kernel
            .into_iter()
            .flat_map(|kernel| IsaTier::supported().map(move |tier| Self::Kernel { kernel, tier }));
        iter::once(Self::Scalar).chain(kernels)
    }

    /// The variant's part of a case id: `scalar`, or the tier's.
    pub(crate) const fn id(self) -> &'static str {
        match self {
            Self::Scalar => "scalar",
            Self::Kernel { tier, .. } => tier.id(),
        }
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
