//! 3×3 median filter for the artifacts a demosaic leaves: its interpolation can leave structure a
//! single pixel wide, which a detection plane of a demosaiced frame passes through this first.

pub(super) mod simd;

use crate::simd::Kernel;
use crate::star_detection::median_filter::simd::MedianRow;

/// One row's 3×3 neighbourhood: the row, and the rows above and below it where the image has them.
///
/// Border pixels are filtered over the 4 or 6 neighbours that exist rather than over a window
/// padded out to 9 — no sample is duplicated. That is why `cosmic_ray`'s `median_window` is a
/// separate filter instead of calling this at `r == 1`: L.A.Cosmic differences two window sizes
/// against each other and so needs one border convention across all of them, and it picked
/// replication.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MedianRows<'a> {
    pub(crate) above: Option<&'a [f32]>,
    pub(crate) cur: &'a [f32],
    pub(crate) below: Option<&'a [f32]>,
}

impl MedianRows<'_> {
    /// The 3×3 median of the row, into `output`: the vector kernel for every pixel with a full
    /// 9-element neighbourhood, and the edge pixels from the neighbours they have. The image is
    /// 3 × 3 at least; a smaller one is left as it is, by the caller.
    pub(crate) fn filter_into(self, output: &mut [f32]) {
        let width = self.cur.len();
        debug_assert!(width >= 3, "a median row is 3 pixels wide at least");
        debug_assert!(
            self.above.is_some() || self.below.is_some(),
            "a median image is 3 rows tall at least"
        );
        let (Some(above), Some(below)) = (self.above, self.below) else {
            for (x, out) in output.iter_mut().enumerate() {
                *out = self.median_at_edge(x);
            }
            return;
        };
        output[0] = median_at_side([above, self.cur, below], 0);
        MedianRow {
            above,
            cur: self.cur,
            below,
            output,
        }
        .dispatch();
        output[width - 1] = median_at_side([above, self.cur, below], width - 2);
    }

    /// The median at `x` of a top or bottom row, over the 4 or 6 neighbours it has.
    fn median_at_edge(self, x: usize) -> f32 {
        let mut neighbors = [0.0f32; 9];
        let mut count = 0;
        let columns = x.saturating_sub(1)..(x + 2).min(self.cur.len());
        for row in [self.above, Some(self.cur), self.below]
            .into_iter()
            .flatten()
        {
            for &sample in &row[columns.clone()] {
                neighbors[count] = sample;
                count += 1;
            }
        }
        // Edge rows span exactly 2 input rows × {2, 3} columns, so the neighborhood is always 4 or 6.
        match count {
            4 => median4(&mut neighbors[..4]),
            6 => median6(&mut neighbors[..6]),
            _ => unreachable!("edge/corner neighborhood is always 4 or 6, got {count}"),
        }
    }
}

/// The median at the left or right end of an interior row: over columns `first` and `first + 1`
/// of the three `rows`.
#[inline]
fn median_at_side(rows: [&[f32]; 3], first: usize) -> f32 {
    let mut v = [0.0f32; 6];
    for (pair, row) in v.as_chunks_mut::<2>().0.iter_mut().zip(rows) {
        pair.copy_from_slice(&row[first..first + 2]);
    }
    median6(&mut v)
}

/// Median of 4 elements (average of middle two).
#[inline]
fn median4(v: &mut [f32]) -> f32 {
    if v[0] > v[1] {
        v.swap(0, 1);
    }
    if v[2] > v[3] {
        v.swap(2, 3);
    }
    if v[0] > v[2] {
        v.swap(0, 2);
    }
    if v[1] > v[3] {
        v.swap(1, 3);
    }
    if v[1] > v[2] {
        v.swap(1, 2);
    }
    f32::midpoint(v[1], v[2])
}

/// Median of 6 elements (average of middle two), through the optimal 12-comparator sorting network
/// for six inputs (Knuth, TAOCP vol. 3, §5.3.4), which sorts every one of the 720 orders.
#[inline]
fn median6(v: &mut [f32]) -> f32 {
    for (i, j) in [
        (1, 2),
        (4, 5),
        (0, 2),
        (3, 5),
        (0, 1),
        (3, 4),
        (1, 4),
        (0, 3),
        (2, 5),
        (1, 3),
        (2, 4),
        (2, 3),
    ] {
        if v[i] > v[j] {
            v.swap(i, j);
        }
    }
    f32::midpoint(v[2], v[3])
}

#[cfg(test)]
pub(crate) mod internals {
    use imaginarium::Buffer2;
    use rayon::prelude::*;

    use crate::math::size2us::Size2us;
    use crate::star_detection::median_filter::MedianRows;

    impl<'a> MedianRows<'a> {
        /// Row `y` of `pixels` and its neighbours.
        pub(crate) fn of(pixels: &'a Buffer2<f32>, y: usize) -> Self {
            Self {
                above: y.checked_sub(1).map(|above| pixels.row(above)),
                cur: pixels.row(y),
                below: (y + 1 < pixels.height()).then(|| pixels.row(y + 1)),
            }
        }
    }

    /// The 3×3 median of `pixels`, into `output`, a row at a time; an image under 3 × 3 is left as
    /// it is.
    pub(crate) fn apply_3x3(pixels: &Buffer2<f32>, output: &mut Buffer2<f32>) {
        let size = Size2us::from(pixels);
        debug_assert_eq!(size, Size2us::from(&*output));
        if size.width < 3 || size.height < 3 {
            output.copy_from_slice(pixels);
            return;
        }
        output
            .pixels_mut()
            .par_chunks_mut(size.width)
            .enumerate()
            .for_each(|(y, row)| MedianRows::of(pixels, y).filter_into(row));
    }
}

#[cfg(test)]
mod tests;
