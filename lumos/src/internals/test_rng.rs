//! [`TestRng`]: the seeded generator every synthetic fixture draws from.

use common::Libm;
use std::f64::consts::TAU;

/// Deterministic Knuth LCG, `state = state * 6364136223846793005 + 1`, for reproducible test data.
#[derive(Debug, Clone)]
pub(crate) struct TestRng {
    state: u64,
}

impl TestRng {
    pub(crate) const fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// Advance state and return raw u64.
    #[inline]
    pub(crate) const fn next_u64(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        self.state
    }

    /// A random f32 in [0, 1): the top 24 bits, which an f32 holds exactly,
    /// scaled by 2⁻²⁴. More bits would round the largest values up to 1.0.
    #[inline]
    pub(crate) fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 * (1.0 / (1u32 << 24) as f32)
    }

    /// A random f64 in [0, 1) with full 53-bit mantissa precision.
    #[inline]
    pub(crate) fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// A standard normal f32 by Box-Muller from two [`Self::next_f32`] draws, combined in `f64`.
    #[inline]
    pub(crate) fn next_gaussian_f32(&mut self) -> f32 {
        let u1 = f64::from(self.next_f32().max(1e-10));
        let u2 = f64::from(self.next_f32());
        ((-2.0 * u1.libm_ln()).sqrt() * (TAU * u2).libm_cos()) as f32
    }
}

#[cfg(test)]
mod tests {
    use crate::internals::test_rng::TestRng;

    /// The largest state the generator can hand out maps to 1 − 2⁻²⁴, below 1.0.
    #[test]
    fn next_f32_stays_below_one() {
        let top = (u64::MAX >> 40) as f32 * (1.0 / (1u32 << 24) as f32);
        assert_eq!(top, 1.0 - 1.0 / (1u32 << 24) as f32);
        let mut rng = TestRng::new(7);
        assert!((0..100_000).all(|_| (0.0..1.0).contains(&rng.next_f32())));
    }
}
