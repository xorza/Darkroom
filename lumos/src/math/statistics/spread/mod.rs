//! [`Spread`]: a robust location and scale, and the floor below which a measured scale says
//! nothing.

use crate::math::statistics;

/// The median of a sorted window and its MAD in Gaussian units, unbiased at every count.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Spread {
    pub(crate) centre: f32,
    pub(crate) sigma: f32,
}

/// The spacing of `f32` at 0.
const SMALLEST_STEP: f32 = f32::from_bits(1);

impl Spread {
    /// Measure an ascending window. One sample has no spread.
    pub(crate) fn of_sorted(sorted: &[f32]) -> Self {
        debug_assert!(!sorted.is_empty());
        debug_assert!(sorted.is_sorted());
        let centre = Self::median_of_sorted(sorted);
        Self {
            centre,
            sigma: statistics::sigma_of_mad(mad_of_sorted(sorted, centre), sorted.len()),
        }
    }

    /// The median of an ascending window: the middle sample, or the mean of the two middle ones.
    /// The upper-middle sample alone sits high by up to half the gap between the two, which makes a
    /// symmetric band about it clip the low side harder.
    pub(crate) fn median_of_sorted(sorted: &[f32]) -> f32 {
        let n = sorted.len();
        debug_assert!(n > 0);
        if n % 2 == 1 {
            sorted[n / 2]
        } else {
            f32::midpoint(sorted[n / 2 - 1], sorted[n / 2])
        }
    }

    /// The smallest spread data at `centre` can show: one `f32` step there, `|centre|·ε` within a
    /// factor of two, and the smallest subnormal at 0. A smaller σ is not measurable whatever span
    /// the decoder divided by, which is what makes the floor scale-free where a fixed
    /// `f32::EPSILON` declares every 32-bit integer FITS flat.
    pub(crate) const fn resolution(centre: f32) -> f32 {
        (centre.abs() * f32::EPSILON).max(SMALLEST_STEP)
    }

    /// The σ to scale a band by: the measured one, raised to `background` and to the resolution at
    /// the centre. Always positive.
    ///
    /// `background` is the noise the frames were measured to have away from this window. A
    /// per-window MAD cannot see it when more than half the samples tie, as integer data with
    /// little noise does, and an outlier cannot raise it. With no background, a band about a tied
    /// majority holds the tied samples alone.
    pub(crate) const fn floored(self, background: f32) -> f32 {
        self.sigma
            .max(background)
            .max(Self::resolution(self.centre))
    }
}

/// The MAD about `centre` of an ascending window, without a scratch buffer: the deviations form two
/// ascending runs, the samples below `centre` read backwards and the rest read forwards, so the
/// middle rank of their union is found by a binary search over how many of it each run gives, in
/// as many steps as the window's length has bits. An even count takes the mean of the two middle
/// deviations, as [`Spread::median_of_sorted`] does for the samples.
fn mad_of_sorted(sorted: &[f32], centre: f32) -> f32 {
    let n = sorted.len();
    let split = sorted.partition_point(|&v| v < centre);
    let below = Run {
        at: |i: usize| centre - sorted[split - 1 - i],
        len: split,
    };
    let above = Run {
        at: |j: usize| sorted[split + j] - centre,
        len: n - split,
    };
    let middle = Ranked::of(&below, &above, (n - 1) / 2);
    if n.is_multiple_of(2) {
        f32::midpoint(middle.value, middle.next)
    } else {
        middle.value
    }
}

/// An ascending run of `len` values, the `i`-th read by `at(i)`.
#[derive(Debug)]
struct Run<F> {
    at: F,
    len: usize,
}

impl<F: Fn(usize) -> f32> Run<F> {
    /// The last of the run's first `count` values; `−∞` when it gives none.
    fn last_taken(&self, count: usize) -> f32 {
        if count > 0 {
            (self.at)(count - 1)
        } else {
            f32::NEG_INFINITY
        }
    }

    /// The run's value after its first `count`; `+∞` when none is left.
    fn first_left(&self, count: usize) -> f32 {
        if count < self.len {
            (self.at)(count)
        } else {
            f32::INFINITY
        }
    }
}

/// The value at one rank of the union of two ascending runs, and the value at the rank after it.
#[derive(Debug, Clone, Copy)]
struct Ranked {
    value: f32,
    /// `+∞` when the rank is the union's last.
    next: f32,
}

impl Ranked {
    /// Rank `rank` of the union of `a` and `b`: the `rank + 1` smallest take some `i` from `a` and
    /// the rest from `b`, and the split is the first `i` whose next `a` is not below the last `b`
    /// it leaves out.
    fn of(a: &Run<impl Fn(usize) -> f32>, b: &Run<impl Fn(usize) -> f32>, rank: usize) -> Self {
        debug_assert!(rank < a.len + b.len);
        let taken = rank + 1;
        let (mut low, mut high) = (taken.saturating_sub(b.len), taken.min(a.len));
        while low < high {
            let i = usize::midpoint(low, high);
            if (a.at)(i) < (b.at)(taken - i - 1) {
                low = i + 1;
            } else {
                high = i;
            }
        }
        let (i, j) = (low, taken - low);
        Self {
            value: a.last_taken(i).max(b.last_taken(j)),
            next: a.first_left(i).min(b.first_left(j)),
        }
    }
}

#[cfg(test)]
mod tests;
