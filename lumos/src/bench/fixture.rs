//! The sampling policy every group takes, and the inputs the benches measure on: drawn from fixed
//! seeds, so two runs time the same work.

use std::f32::consts::TAU;
use std::time::Duration;

use criterion::measurement::WallTime;
use criterion::{BenchmarkGroup, Criterion};
use rand::prelude::*;
use rand_chacha::ChaCha8Rng;

/// How much one iteration of a group's benches processes, which sets how criterion samples it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scale {
    /// Up to tens of thousands of elements: microseconds an iteration.
    Sample,
    /// A whole frame or plane: milliseconds an iteration, so fewer and longer samples.
    Frame,
}

/// The group `name`, sampled as `scale` says.
pub(crate) fn group<'a>(
    c: &'a mut Criterion,
    name: &str,
    scale: Scale,
) -> BenchmarkGroup<'a, WallTime> {
    let mut group = c.benchmark_group(name);
    match scale {
        Scale::Sample => group
            .sample_size(50)
            .warm_up_time(Duration::from_millis(500))
            .measurement_time(Duration::from_secs(1)),
        Scale::Frame => group
            .sample_size(10)
            .warm_up_time(Duration::from_secs(1))
            .measurement_time(Duration::from_secs(3)),
    };
    group
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
