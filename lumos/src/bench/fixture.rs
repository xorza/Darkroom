//! The sampling policy every group takes, and the inputs the benches measure on: drawn from fixed
//! seeds, so two runs time the same work.

use std::f32::consts::TAU;
use std::time::Duration;

use criterion::measurement::WallTime;
use criterion::{BatchSize, BenchmarkGroup, Criterion, SamplingMode};
use rand::prelude::*;
use rand_chacha::ChaCha8Rng;

/// What a bench times, the first part of its id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// One operation on the unit production gives it.
    Atomic,
    /// A pipeline stage on a frame or a set of them, or a public entry point end to end.
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

/// How much one iteration of a group's benches processes, which sets how criterion samples it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scale {
    /// Up to tens of thousands of elements: microseconds an iteration.
    Sample,
    /// A whole frame or plane: milliseconds an iteration, so fewer and longer samples.
    Frame,
    /// A set of frames, in memory or in files: seconds an iteration, sampled flat at the fewest
    /// samples criterion takes, two iterations each. `iteration` is the group's slowest bench as
    /// measured, which sets the measurement time.
    Set { iteration: Duration },
}

impl Scale {
    /// The samples of a [`Scale::Set`] group: the fewest criterion takes.
    const SET_SAMPLES: u32 = 10;

    /// The iterations each sample of a [`Scale::Set`] group takes at the measured iteration.
    /// Criterion's flat sampling warns of a sample of one iteration, which it takes wherever the
    /// measurement time is at most the samples times the iteration: at two, only a bench that
    /// runs at twice the measured iteration or slower takes one.
    const SET_ITERATIONS: u32 = 2;

    /// Sets the sampling of the benches `group` registers from here on.
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

/// The batch for a routine that changes an input of `bytes` in place or consumes it, so takes a
/// fresh copy each iteration: as many copies as fit in 128 KiB, at least one. Criterion builds a
/// batch's copies before it times them, so a batch that outgrows the cache times the routine on
/// inputs memory has to bring back.
pub(crate) const fn fresh_copies(bytes: usize) -> BatchSize {
    let copies = (128 * 1024) / if bytes == 0 { 1 } else { bytes };
    BatchSize::NumIterations(if copies == 0 { 1 } else { copies as u64 })
}

/// `len` draws of a unit normal, by Box–Muller.
pub(crate) fn gaussian(len: usize, seed: u64) -> Vec<f32> {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    (0..len)
        .map(|_| {
            let (u, v) = (1.0 - rng.random::<f32>(), rng.random::<f32>());
            (-2.0 * u.ln()).sqrt() * (TAU * v).cos()
        })
        .collect()
}

/// `len` samples of sky: 0.1 with white noise of σ 0.01, and one in fifty a star up to 1 above it.
pub(crate) fn sky(len: usize, seed: u64) -> Vec<f32> {
    let mut rng = ChaCha8Rng::seed_from_u64(seed ^ 0x5EED);
    gaussian(len, seed)
        .into_iter()
        .map(|noise| {
            let star = if rng.random_range(0..50) == 0 {
                rng.random::<f32>()
            } else {
                0.0
            };
            0.1 + 0.01 * noise + star
        })
        .collect()
}
