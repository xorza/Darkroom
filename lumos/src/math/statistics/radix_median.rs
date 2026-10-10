//! [`RadixMedian`]: the exact median of values read twice, in fixed memory.
//!
//! A median by selection needs the values in one buffer, which for a full plane is the plane
//! again. Ranking the values by their bits instead needs only a histogram: the first pass counts
//! the high half of each value's key and finds the bin that holds the middle rank, the second
//! counts the low half of the values in that bin. The keys order the values as
//! [`f32::total_cmp`] does, so the middle values are the ones
//! [`median_mut`](crate::math::statistics::median_mut) selects, and the median is bit for bit its
//! own wherever the two middles' sum stays finite: [`f32::midpoint`] also averages two values near
//! the largest float, where `median_mut`'s sum overflows.

/// Bins per pass: one for each value of a 16-bit half of a key.
const BINS: usize = 1 << 16;

/// The histogram both passes count into, kept across medians: 256 KiB, allocated by the first
/// median, so a caller that may need none holds none.
#[derive(Debug, Default)]
pub(crate) struct RadixMedian {
    counts: Vec<u32>,
}

/// The first pass: every value counted by the high half of its key.
#[derive(Debug)]
pub(crate) struct HighPass<'a> {
    counts: &'a mut [u32],
    len: usize,
}

/// The second pass: the values of the bins that hold the middle ranks.
#[derive(Debug)]
pub(crate) struct LowPass<'a> {
    counts: &'a mut [u32],
    middle: Middle,
}

/// Where the middle ranks fell in the first pass.
#[derive(Debug, Clone, Copy)]
enum Middle {
    /// Both middle ranks, or the one of an odd count, in the bin `high` of `len` values: the second
    /// pass counts them by the low half of their key, and `rank` is the lower middle's rank among
    /// them.
    OneBin {
        high: u32,
        len: u32,
        rank: u32,
        even: bool,
    },
    /// The lower middle is the greatest value of bin `lower`, and the upper middle the least of
    /// the next occupied bin, `upper`.
    Split {
        lower: u32,
        upper: u32,
        lower_max: u32,
        upper_min: u32,
    },
}

impl RadixMedian {
    /// Start a median: feed every value to the pass this returns, then every value again to the
    /// pass that one finishes into.
    pub(crate) fn high_pass(&mut self) -> HighPass<'_> {
        self.counts.clear();
        self.counts.resize(BINS, 0);
        HighPass {
            counts: &mut self.counts,
            len: 0,
        }
    }
}

impl<'a> HighPass<'a> {
    #[inline]
    pub(crate) const fn add(&mut self, value: f32) {
        self.counts[(key(value) >> 16) as usize] += 1;
        self.len += 1;
    }

    /// The values counted so far.
    pub(crate) const fn len(&self) -> usize {
        self.len
    }

    /// Locate the middle ranks for the second pass.
    pub(crate) fn finish(self) -> LowPass<'a> {
        let Self { counts, len } = self;
        assert!(len > 0, "the median of no values");
        assert!(
            u32::try_from(len).is_ok(),
            "{len} values overflow a bin count"
        );
        let even = len % 2 == 0;
        let lower_rank = if even { len / 2 - 1 } else { len / 2 };
        let (lower, below) = bin_of(counts, lower_rank);
        let in_bin = counts[lower as usize] as usize;
        let middle = if !even || lower_rank + 1 < below + in_bin {
            counts.fill(0);
            Middle::OneBin {
                high: lower,
                len: in_bin as u32,
                rank: (lower_rank - below) as u32,
                even,
            }
        } else {
            let upper = (lower + 1..BINS as u32)
                .find(|&bin| counts[bin as usize] > 0)
                .expect("the upper middle lies above the lower");
            Middle::Split {
                lower,
                upper,
                lower_max: 0,
                upper_min: u32::MAX,
            }
        };
        LowPass { counts, middle }
    }
}

impl LowPass<'_> {
    #[inline]
    pub(crate) fn add(&mut self, value: f32) {
        let key = key(value);
        match &mut self.middle {
            Middle::OneBin { high, .. } => {
                if key >> 16 == *high {
                    self.counts[(key & 0xFFFF) as usize] += 1;
                }
            }
            Middle::Split {
                lower,
                upper,
                lower_max,
                upper_min,
            } => {
                let high = key >> 16;
                if high == *lower {
                    *lower_max = (*lower_max).max(key);
                } else if high == *upper {
                    *upper_min = (*upper_min).min(key);
                }
            }
        }
    }

    /// The median of the values both passes saw: the middle one of an odd count, the mean of the
    /// two middle ones of an even count.
    pub(crate) fn median(self) -> f32 {
        let (lower, upper) = match self.middle {
            Middle::OneBin {
                high,
                len,
                rank,
                even,
            } => {
                debug_assert_eq!(
                    self.counts.iter().sum::<u32>(),
                    len,
                    "the second pass saw other values than the first"
                );
                let (low, below) = bin_of(self.counts, rank as usize);
                let lower = high << 16 | low;
                if !even {
                    return value(lower);
                }
                let upper = if (rank as usize) + 1 < below + self.counts[low as usize] as usize {
                    lower
                } else {
                    let next = (low + 1..BINS as u32)
                        .find(|&bin| self.counts[bin as usize] > 0)
                        .expect("the upper middle lies in the same bin");
                    high << 16 | next
                };
                (lower, upper)
            }
            Middle::Split {
                lower,
                upper,
                lower_max,
                upper_min,
            } => {
                debug_assert!(
                    lower_max >> 16 == lower && upper_min >> 16 == upper,
                    "the second pass saw other values than the first"
                );
                (lower_max, upper_min)
            }
        };
        f32::midpoint(value(lower), value(upper))
    }
}

/// The bin that holds the value of `rank`, and the count of values in the bins below it.
fn bin_of(counts: &[u32], rank: usize) -> (u32, usize) {
    let mut below = 0;
    for (bin, &count) in counts.iter().enumerate() {
        let count = count as usize;
        if rank < below + count {
            return (bin as u32, below);
        }
        below += count;
    }
    unreachable!("rank {rank} lies past the {below} values counted")
}

/// The value's bits mapped so unsigned order is [`f32::total_cmp`]'s: a negative value's bits all
/// flip, a positive value's sign bit sets.
#[inline]
const fn key(value: f32) -> u32 {
    let bits = value.to_bits();
    if bits >> 31 == 1 {
        !bits
    } else {
        bits | 1 << 31
    }
}

/// The value whose [`key`] is `key`.
#[inline]
const fn value(key: u32) -> f32 {
    f32::from_bits(if key >> 31 == 1 {
        key & !(1 << 31)
    } else {
        !key
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::statistics::median_mut;

    fn radix_median(values: &[f32]) -> f32 {
        let mut median = RadixMedian::default();
        let mut high = median.high_pass();
        for &value in values {
            high.add(value);
        }
        let mut low = high.finish();
        for &value in values {
            low.add(value);
        }
        low.median()
    }

    /// The key orders values as `total_cmp` does, and maps back to the same bits.
    #[test]
    fn keys_follow_the_total_order() {
        let ordered = [
            f32::NEG_INFINITY,
            -1.5,
            -f32::MIN_POSITIVE,
            -0.0,
            0.0,
            f32::MIN_POSITIVE,
            1.5,
            f32::INFINITY,
        ];
        for pair in ordered.windows(2) {
            assert!(key(pair[0]) < key(pair[1]), "{pair:?}");
        }
        for number in ordered {
            assert_eq!(value(key(number)).to_bits(), number.to_bits());
        }
    }

    /// Hand-ranked cases for each way the middle ranks fall:
    /// - odd: [3, −1, 2] has the middle 2;
    /// - even in one bin: 1.0 and 1.0000001 differ only in the low half, so (1 + 1.0000001)/2;
    /// - even across bins: [−2, 4] has the mean 1;
    /// - even across bins with empty bins between them: [0.25, 8, 1, 1000] has the mean of 1 and 8;
    /// - a constant set, every value in one bin and one low bin;
    /// - one value.
    #[test]
    fn the_median_falls_where_the_ranks_do() {
        let next = f32::from_bits(1.0f32.to_bits() + 1);
        for (values, expected) in [
            (vec![3.0, -1.0, 2.0], 2.0),
            (vec![next, 1.0], f32::midpoint(1.0, next)),
            (vec![-2.0, 4.0], 1.0),
            (vec![0.25, 8.0, 1.0, 1000.0], 4.5),
            (vec![0.5; 6], 0.5),
            (vec![-7.0], -7.0),
        ] {
            assert_eq!(
                radix_median(&values).to_bits(),
                expected.to_bits(),
                "{values:?}"
            );
        }
    }

    /// Cross-check against `median_mut` on sets that put the middle ranks in every position, all
    /// far from overflow:
    /// signed values on both sides of zero, both zeros, runs of duplicates, and counts from 1 to
    /// 400, odd and even. The two agree bit for bit.
    #[test]
    fn the_median_is_bit_identical_to_selection() {
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for len in 1..=400 {
            let values: Vec<f32> = (0..len)
                .map(|_| match next() % 5 {
                    0 => 0.0,
                    1 => -0.0,
                    2 => (next() % 8) as f32 * 0.125,
                    _ => (next() as i32) as f32 * 1e-9,
                })
                .collect();
            let expected = median_mut(&mut values.clone());
            assert_eq!(
                radix_median(&values).to_bits(),
                expected.to_bits(),
                "{len} values"
            );
        }
    }
}
