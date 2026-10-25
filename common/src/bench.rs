//! The bench policy every crate's benches share: the kind of a case, the first part of its id, and
//! the sampling criterion takes for how long one iteration runs.

use std::time::Duration;

use criterion::measurement::WallTime;
use criterion::{BatchSize, BenchmarkGroup, Criterion, SamplingMode};

/// What a case times, the first part of its id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// One operation on the unit production hands it.
    Atomic,
    /// A stage or a public entry point, end to end.
    Integration,
}

impl Kind {
    /// The group `name` of this kind, sampled as `scale` says.
    pub fn group<'a>(
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

/// How long one iteration of a group's cases runs, which sets how criterion samples it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scale {
    /// Microseconds an iteration.
    Micros,
    /// Milliseconds an iteration, so fewer and longer samples.
    Millis,
    /// Hundreds of milliseconds an iteration or more: sampled flat at the fewest samples
    /// criterion takes, two iterations each. `iteration` is the group's slowest case as measured,
    /// which sets the measurement time.
    Slow { iteration: Duration },
}

impl Scale {
    /// The samples of a [`Scale::Slow`] group: the fewest criterion takes.
    const SLOW_SAMPLES: u32 = 10;

    /// The iterations each sample of a [`Scale::Slow`] group takes at the measured iteration.
    /// Criterion's flat sampling warns of a sample of one iteration, which it takes wherever the
    /// measurement time is at most the samples times the iteration: at two, only a case that runs
    /// at twice the measured iteration or slower takes one.
    const SLOW_ITERATIONS: u32 = 2;

    /// Sets the sampling of the cases `group` registers from here on.
    #[expect(
        clippy::disallowed_methods,
        reason = "`Scale::apply` is the one place a group's sampling is set"
    )]
    pub fn apply(self, group: &mut BenchmarkGroup<'_, WallTime>) {
        match self {
            Self::Micros => group
                .sample_size(50)
                .sampling_mode(SamplingMode::Auto)
                .warm_up_time(Duration::from_millis(500))
                .measurement_time(Duration::from_secs(1)),
            Self::Millis => group
                .sample_size(10)
                .sampling_mode(SamplingMode::Auto)
                .warm_up_time(Duration::from_secs(1))
                .measurement_time(Duration::from_secs(3)),
            Self::Slow { iteration } => group
                .sample_size(Self::SLOW_SAMPLES as usize)
                .sampling_mode(SamplingMode::Flat)
                .warm_up_time(Duration::from_secs(1))
                .measurement_time(iteration * Self::SLOW_SAMPLES * Self::SLOW_ITERATIONS),
        };
    }
}

/// The batch for a routine that changes an input of `bytes` in place or consumes it, so takes a
/// fresh copy each iteration: as many copies as fit in 128 KiB, at least one. Criterion builds a
/// batch's copies before it times them, so a batch that outgrows the cache times the routine on
/// inputs memory has to bring back.
pub const fn fresh_copies(bytes: usize) -> BatchSize {
    let copies = (128 * 1024) / if bytes == 0 { 1 } else { bytes };
    BatchSize::NumIterations(if copies == 0 { 1 } else { copies as u64 })
}
