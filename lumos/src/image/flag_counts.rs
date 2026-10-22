//! [`FlagCounts`]: how many pixels or samples hold each quality flag.

use std::sync::atomic::{AtomicU64, Ordering};

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::image::quality_flags::QualityFlags;

/// How many pixels or samples hold each of the [`QualityFlags`]. One that holds two flags counts
/// under both.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlagCounts([u64; 8]);

impl FlagCounts {
    /// The counts of `per_bit`, entry `i` under the flag `1 << i`.
    pub(crate) const fn from_per_bit(per_bit: [u64; 8]) -> Self {
        Self(per_bit)
    }

    /// Each count, entry `i` under the flag `1 << i`.
    pub(crate) const fn per_bit(&self) -> &[u64; 8] {
        &self.0
    }

    /// How many hold `flag`.
    ///
    /// # Panics
    ///
    /// If `flag` is not a single flag.
    pub const fn count(&self, flag: QualityFlags) -> u64 {
        self.0[flag.idx()]
    }

    /// Whether any holds a flag of `flags`.
    pub const fn any_of(&self, flags: QualityFlags) -> bool {
        let mut bit = 0;
        while bit < self.0.len() {
            if self.0[bit] > 0 && flags.byte() & (1 << bit) != 0 {
                return true;
            }
            bit += 1;
        }
        false
    }

    /// Whether any holds a flag at all.
    pub(crate) const fn any(&self) -> bool {
        self.any_of(QualityFlags::from_byte(u8::MAX))
    }

    /// Whether any holds a flag other than those in `except`.
    pub(crate) const fn any_other_than(&self, except: QualityFlags) -> bool {
        self.any_of(QualityFlags::from_byte(!except.byte()))
    }

    /// Count one more under each flag `flags` holds.
    #[inline]
    pub(crate) const fn tally(&mut self, flags: QualityFlags) {
        let mut remaining = flags.byte();
        while remaining != 0 {
            self.0[remaining.trailing_zeros() as usize] += 1;
            remaining &= remaining - 1;
        }
    }

    /// The two counts together.
    pub(crate) const fn merged(mut self, other: Self) -> Self {
        let mut bit = 0;
        while bit < self.0.len() {
            self.0[bit] += other.0[bit];
            bit += 1;
        }
        self
    }

    /// The counts of `bytes`, each a pixel's flags, in parallel spans.
    pub(crate) fn of(bytes: &[u8]) -> Self {
        bytes
            .par_chunks(1 << 16)
            .map(|span| {
                let mut counts = Self::default();
                for &byte in span {
                    counts.tally(QualityFlags::from_byte(byte));
                }
                counts
            })
            .reduce(Self::default, Self::merged)
    }
}

/// [`FlagCounts`] that parallel workers add their own into.
#[derive(Debug, Default)]
pub(crate) struct AtomicFlagCounts([AtomicU64; 8]);

impl AtomicFlagCounts {
    pub(crate) fn add(&self, local: &FlagCounts) {
        for (total, &count) in self.0.iter().zip(&local.0) {
            if count > 0 {
                total.fetch_add(count, Ordering::Relaxed);
            }
        }
    }

    pub(crate) fn totals(&self) -> FlagCounts {
        FlagCounts(self.0.each_ref().map(|count| count.load(Ordering::Relaxed)))
    }
}

#[cfg(test)]
mod tests {
    use crate::image::flag_counts::{AtomicFlagCounts, FlagCounts};
    use crate::image::quality_flags::QualityFlags;

    /// Each flag a pixel holds counts once: of the bytes `[SATURATED | DEFECT, DEFECT, 0,
    /// COSMIC_RAY | DEFECT]`, DEFECT is in 3, SATURATED and COSMIC_RAY in 1 each, and NO_DATA in
    /// none. Two workers' counts add up through the atomic form to twice each, and `any_of` sees a
    /// flag only where it was counted.
    #[test]
    fn each_flag_a_pixel_holds_counts_once() {
        let bytes = [
            QualityFlags::SATURATED.union(QualityFlags::DEFECT),
            QualityFlags::DEFECT,
            QualityFlags::default(),
            QualityFlags::COSMIC_RAY.union(QualityFlags::DEFECT),
        ]
        .map(QualityFlags::byte);
        let counts = FlagCounts::of(&bytes);
        let expected = [
            (QualityFlags::DEFECT, 3),
            (QualityFlags::SATURATED, 1),
            (QualityFlags::COSMIC_RAY, 1),
            (QualityFlags::NO_DATA, 0),
        ];
        for (flag, count) in expected {
            assert_eq!(counts.count(flag), count, "{flag:?}");
        }
        assert!(counts.any());
        assert!(counts.any_of(QualityFlags::NO_DATA.union(QualityFlags::COSMIC_RAY)));
        assert!(!counts.any_of(QualityFlags::NO_DATA.union(QualityFlags::REPAIRED)));
        assert!(!FlagCounts::default().any());

        let totals = AtomicFlagCounts::default();
        totals.add(&counts);
        totals.add(&counts);
        assert_eq!(totals.totals(), counts.merged(counts));
        for (flag, count) in expected {
            assert_eq!(totals.totals().count(flag), 2 * count, "{flag:?}");
        }
    }
}
