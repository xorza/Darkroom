//! [`Rng`]: xoshiro256**, the workspace's one pseudo-random generator, for sampling and fixtures,
//! never secrets. Its authors' recommendation for general-purpose 64-bit output, seeded through
//! SplitMix64 as they advise ([Vigna](https://prng.di.unimi.it/)); integer arithmetic alone, so a
//! seed draws the same values on every platform and in every build. Refused: `rand` and
//! `rand_chacha`, four crates for a seeded stream and a uniform draw.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher as _, Hasher as _};
use std::ops::Range;

/// A xoshiro256** stream: 256 bits of state, a period of 2²⁵⁶ − 1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rng {
    state: [u64; 4],
}

impl Rng {
    /// The stream `seed` names: its four words drawn from SplitMix64 at `seed`, which never gives
    /// the all-zero state xoshiro cannot leave.
    pub const fn seeded(seed: u64) -> Self {
        let mut splitmix = seed;
        let mut state = [0; 4];
        let mut idx = 0;
        while idx < state.len() {
            splitmix = splitmix.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = splitmix;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            state[idx] = z ^ (z >> 31);
            idx += 1;
        }
        Self { state }
    }

    /// A stream no two processes share: seeded from a `RandomState`'s keys, which std draws from
    /// the operating system's randomness, as `fastrand` seeds itself.
    pub fn from_entropy() -> Self {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u64(0);
        Self::seeded(hasher.finish())
    }

    /// The next 64 bits.
    pub const fn next_u64(&mut self) -> u64 {
        let [s0, s1, s2, s3] = self.state;
        let out = s1.wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = s1 << 17;
        let s2 = s2 ^ s0;
        let s3 = s3 ^ s1;
        let s1 = s1 ^ s2;
        let s0 = s0 ^ s3;
        self.state = [s0, s1, s2 ^ t, s3.rotate_left(45)];
        out
    }

    /// A value in `[0, 1)`, each of the 2⁵³ multiples of 2⁻⁵³ alike.
    #[expect(
        clippy::cast_precision_loss,
        reason = "53 bits are exact in an f64, the cast loses nothing"
    )]
    pub const fn unit_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// A value in `[0, 1)`, each of the 2²⁴ multiples of 2⁻²⁴ alike.
    #[expect(
        clippy::cast_precision_loss,
        reason = "24 bits are exact in an f32, the cast loses nothing"
    )]
    pub const fn unit_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 * (1.0 / (1u32 << 24) as f32)
    }

    /// An index in `range`, each alike: Lemire's multiply-and-reject, unbiased, as `rand`'s
    /// uniform integers are ([Lemire](https://arxiv.org/abs/1805.10941)).
    ///
    /// # Panics
    ///
    /// Panics on an empty range.
    #[track_caller]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the result is below the range's length, a usize"
    )]
    pub fn index(&mut self, range: Range<usize>) -> usize {
        assert!(
            range.start < range.end,
            "an index of the empty range {range:?}"
        );
        let span = (range.end - range.start) as u64;
        let mut wide = u128::from(self.next_u64()) * u128::from(span);
        if (wide as u64) < span {
            let floor = span.wrapping_neg() % span;
            while (wide as u64) < floor {
                wide = u128::from(self.next_u64()) * u128::from(span);
            }
        }
        range.start + (wide >> 64) as usize
    }
}

#[cfg(test)]
mod tests {
    use crate::rng::Rng;

    /// From the state `[1, 2, 3, 4]` the stream is xoshiro256**'s reference output, as the
    /// `rand_xoshiro` tests pin it; SplitMix64 at 0 gives its reference words, as `seeded(0)`'s
    /// state.
    #[test]
    fn the_stream_is_the_reference_one() {
        let mut rng = Rng {
            state: [1, 2, 3, 4],
        };
        let drawn: Vec<u64> = (0..10).map(|_| rng.next_u64()).collect();
        assert_eq!(
            drawn,
            [
                11520,
                0,
                1_509_978_240,
                1_215_971_899_390_074_240,
                1_216_172_134_540_287_360,
                607_988_272_756_665_600,
                16_172_922_978_634_559_625,
                8_476_171_486_693_032_832,
                10_595_114_339_597_558_777,
                2_904_607_092_377_533_576,
            ]
        );
        assert_eq!(
            Rng::seeded(0).state,
            [
                0xE220_A839_7B1D_CDAF,
                0x6E78_9E6A_A1B9_65F4,
                0x06C4_5D18_8009_454F,
                0xF88B_B8A8_724C_81EC,
            ]
        );
    }

    /// A unit value is a draw's top 53 or 24 bits over their span: from `[1, 2, 3, 4]` the first
    /// draw, 11520, keeps 5 in its top 53 bits and 0 in its top 24, and the second draw is 0. A
    /// thousand draws stay in `[0, 1)`.
    #[test]
    fn a_unit_value_is_the_top_bits_over_their_span() {
        let mut rng = Rng {
            state: [1, 2, 3, 4],
        };
        assert_eq!(rng.unit_f64(), 5.0 / (1u64 << 53) as f64);
        assert_eq!(rng.unit_f64(), 0.0);
        let mut rng = Rng {
            state: [1, 2, 3, 4],
        };
        assert_eq!(rng.unit_f32(), 0.0);
        let mut rng = Rng::seeded(7);
        for _ in 0..1000 {
            let u = rng.unit_f64();
            assert!((0.0..1.0).contains(&u), "{u}");
            let v = rng.unit_f32();
            assert!((0.0..1.0).contains(&v), "{v}");
        }
    }

    /// Indices stay in their range and reach every value of it: a thousand draws of `3..7` hit
    /// each of 3, 4, 5 and 6, a one-value range gives it, and seeds give their own streams.
    #[test]
    fn an_index_covers_its_range_alone() {
        let mut rng = Rng::seeded(42);
        let mut seen = [0usize; 4];
        for _ in 0..1000 {
            let idx = rng.index(3..7);
            seen[idx - 3] += 1;
        }
        assert!(seen.iter().all(|&count| count > 150), "{seen:?}");
        assert_eq!(rng.index(9..10), 9);
        assert_ne!(Rng::seeded(1).next_u64(), Rng::seeded(2).next_u64());
        assert_ne!(Rng::from_entropy(), Rng::from_entropy());
    }

    /// An empty range is refused.
    #[test]
    #[should_panic(expected = "an index of the empty range 4..4")]
    fn an_empty_range_is_refused() {
        Rng::seeded(0).index(4..4);
    }
}
