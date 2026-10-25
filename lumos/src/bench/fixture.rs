//! The inputs the benches measure on: drawn from fixed seeds, so two runs time the same work.

use std::f32::consts::TAU;

use rand::prelude::*;
use rand_chacha::ChaCha8Rng;

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
