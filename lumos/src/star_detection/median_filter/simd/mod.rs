//! The 3x3 median of a row's interior pixels as a vector kernel.
//!
//! Each lane takes one pixel's nine neighbours through one min/max network, so every lane is that
//! pixel's median whatever the lanes beside it hold. That lets the kernel cover the whole interior
//! with no scalar remainder: the last vector overlaps the one before it and recomputes a few
//! pixels to the same values, and a row narrower than a vector runs one zero-padded vector.

use crate::simd::{F32_LANES, F32x8, Isa, Kernel};

/// The interior of one row, pixels `1..width − 1` of `output`, from the rows above, at and below
/// it, on the widest Isa this CPU has. The edges carry no full window and are left alone.
#[inline]
pub(super) fn median_filter_row(above: &[f32], curr: &[f32], below: &[f32], output: &mut [f32]) {
    MedianRow {
        above,
        curr,
        below,
        output,
    }
    .dispatch();
}

/// [`median_filter_row`] as a kernel.
#[derive(Debug)]
struct MedianRow<'a> {
    above: &'a [f32],
    curr: &'a [f32],
    below: &'a [f32],
    output: &'a mut [f32],
}

impl Kernel for MedianRow<'_> {
    type Output = ();

    #[inline(always)]
    fn run<S: Isa>(self, isa: S) {
        let width = self.output.len();
        debug_assert!(
            [self.above, self.curr, self.below]
                .iter()
                .all(|row| row.len() == width),
            "three rows as wide as the output"
        );
        if width < 3 {
            return;
        }
        let interior = width - 2;
        let rows = [self.above, self.curr, self.below];

        if interior < F32_LANES {
            let mut window = [isa.splat_f32(0.0); 9];
            for (row, taps) in rows.iter().zip(window.as_chunks_mut::<3>().0) {
                for (dx, tap) in taps.iter_mut().enumerate() {
                    *tap = isa.load_f32_partial(&row[dx..dx + interior]);
                }
            }
            median9(window).store_partial(&mut self.output[1..=interior]);
            return;
        }

        let last = width - 1 - F32_LANES;
        let mut x = 1;
        loop {
            let start = x.min(last);
            let mut window = [isa.splat_f32(0.0); 9];
            for (row, taps) in rows.iter().zip(window.as_chunks_mut::<3>().0) {
                for (dx, tap) in taps.iter_mut().enumerate() {
                    *tap = isa.load_f32_at(row, start - 1 + dx);
                }
            }
            median9(window).store(
                self.output[start..]
                    .first_chunk_mut()
                    .expect("a full vector inside the interior"),
            );
            if start == last {
                break;
            }
            x += F32_LANES;
        }
    }
}

/// The median of nine lanes, by a 25-comparator network pruned to the comparators the median
/// reads. Each step is a compare-swap, `(a, b) → (a.min(b), a.max(b))`, or the one half of it the
/// rest of the network uses.
///
/// `median9_scalar` (in the tests) stays a separate, independently written network, so the
/// cross-check validates this one rather than re-running it.
#[inline(always)]
fn median9<V: F32x8>(
    [
        mut v0,
        mut v1,
        mut v2,
        mut v3,
        mut v4,
        mut v5,
        mut v6,
        mut v7,
        mut v8,
    ]: [V; 9],
) -> V {
    compare_swap(&mut v0, &mut v1);
    compare_swap(&mut v3, &mut v4);
    compare_swap(&mut v6, &mut v7);
    compare_swap(&mut v1, &mut v2);
    compare_swap(&mut v4, &mut v5);
    compare_swap(&mut v7, &mut v8);
    compare_swap(&mut v0, &mut v1);
    compare_swap(&mut v3, &mut v4);
    compare_swap(&mut v6, &mut v7);
    v3 = v0.max(v3);
    v6 = v3.max(v6);
    compare_swap(&mut v1, &mut v4);
    compare_swap(&mut v4, &mut v7);
    v4 = v1.max(v4);
    compare_swap(&mut v2, &mut v5);
    v5 = v5.min(v8);
    compare_swap(&mut v2, &mut v5);
    v5 = v5.min(v7);
    compare_swap(&mut v2, &mut v6);
    v4 = v4.min(v6);
    v4 = v2.max(v4);
    v4.min(v5)
}

#[inline(always)]
fn compare_swap<V: F32x8>(a: &mut V, b: &mut V) {
    let low = a.min(*b);
    *b = a.max(*b);
    *a = low;
}

#[cfg(test)]
mod internals {
    /// The scalar reference the kernel is tested and benched against: the interior of one row.
    pub(super) fn median_filter_row_scalar(
        row_above: &[f32],
        row_curr: &[f32],
        row_below: &[f32],
        output_row: &mut [f32],
    ) {
        for x in 1..output_row.len().saturating_sub(1) {
            output_row[x] = median9_scalar([
                row_above[x - 1],
                row_above[x],
                row_above[x + 1],
                row_curr[x - 1],
                row_curr[x],
                row_curr[x + 1],
                row_below[x - 1],
                row_below[x],
                row_below[x + 1],
            ]);
        }
    }

    /// Scalar median of 9 elements by an optimal 25-comparator sorting network.
    pub(super) fn median9_scalar(mut v: [f32; 9]) -> f32 {
        const NETWORK: [(usize, usize); 25] = [
            (0, 1),
            (3, 4),
            (6, 7),
            (1, 2),
            (4, 5),
            (7, 8),
            (0, 1),
            (3, 4),
            (6, 7),
            (0, 3),
            (3, 6),
            (0, 3),
            (1, 4),
            (4, 7),
            (1, 4),
            (2, 5),
            (5, 8),
            (2, 5),
            (1, 3),
            (5, 7),
            (2, 6),
            (4, 6),
            (2, 4),
            (2, 3),
            (4, 5),
        ];
        for (a, b) in NETWORK {
            if v[a] > v[b] {
                v.swap(a, b);
            }
        }
        v[4]
    }
}

#[cfg(all(test, feature = "bench"))]
mod bench;
#[cfg(test)]
mod tests;
