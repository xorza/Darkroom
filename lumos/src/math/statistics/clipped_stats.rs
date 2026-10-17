//! [`ClippedStats`]: the sigma-clipped median, σ and mean of a sample.

use std::iter;
use std::ops::Range;

use arrayvec::ArrayVec;

use crate::math::statistics::float::{Float, OrderKeys};
use crate::math::statistics::median_mad::MedianMad;
use crate::math::statistics::spread::Spread;
use crate::math::{statistics, sum};

/// Scratch space a sigma-clip pass borrows for its per-value deviations.
///
/// Exists so one clip implementation serves both buffer kinds: the tiled background walks
/// thousands of tiles and reuses one heap `Vec`, while the centroid measure loop runs per star
/// inside a rayon fold and must not allocate at all. Contents are never read before being
/// written, so an implementation only has to produce a slice of the right length.
pub(crate) trait DeviationScratch {
    /// Yield exactly `len` elements to write into.
    fn sized_to(&mut self, len: usize) -> &mut [f32];
}

impl DeviationScratch for Vec<f32> {
    fn sized_to(&mut self, len: usize) -> &mut [f32] {
        self.resize(len, 0.0);
        self
    }
}

impl<const N: usize> DeviationScratch for ArrayVec<f32, N> {
    /// Grows to `len` rather than starting from a zeroed `[f32; N]`: the caller sizes `N` to its
    /// worst case, so a plain array would memset the whole capacity on every call while only
    /// `len` of it is ever used. Panics when `len > N` — a fixed buffer cannot grow.
    fn sized_to(&mut self, len: usize) -> &mut [f32] {
        self.clear();
        self.extend(iter::repeat_n(0.0f32, len));
        self.as_mut_slice()
    }
}

/// Statistics of the sigma-clip survivors, from [`ClippedStats::sigma_clipped`].
#[derive(Debug, Clone, Copy)]
pub(crate) struct ClippedStats {
    pub(crate) median: f32,
    /// MAD-based sigma of the survivors, with the small-sample factor at their count.
    pub(crate) sigma: f32,
    /// Mean of the survivors. With the median it exposes the residual skew of the clipped
    /// distribution — what SExtractor's Pearson mode estimator corrects for.
    pub(crate) mean: f32,
}

impl ClippedStats {
    /// Widen a clip pass's estimate with the mean of the same survivors.
    fn from_median_mad(location: MedianMad, mean: f32) -> Self {
        Self {
            median: location.median,
            sigma: location.sigma(),
            mean,
        }
    }

    /// Sigma-clipped median, MAD-sigma and mean of `values`, under `clip`.
    ///
    /// `values` must be non-empty and NaN-free, as a NaN never passes the clip's band and so has no
    /// place among the survivors whose mean this reports, and is reordered in place.
    ///
    /// `deviations` is borrowed scratch, overwritten in full; only its length matters. Sizing it
    /// is [`DeviationScratch`]'s job, so a heap caller can reuse one `Vec` across calls and a
    /// stamp-sized caller can keep its scratch on the stack, without this having to know which.
    pub(crate) fn sigma_clipped(
        values: &mut [f32],
        deviations: &mut impl DeviationScratch,
        clip: SigmaClip,
    ) -> Self {
        debug_assert!(!values.is_empty());
        debug_assert!(
            !values.iter().any(|value| value.is_nan()),
            "sigma_clipped requires NaN-free values"
        );

        sigma_clipped_core(values, deviations.sized_to(values.len()), clip)
    }
}

/// How a sigma clip rejects: values beyond `kappa × sigma` from the median, over `iterations`
/// passes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SigmaClip {
    pub(crate) kappa: f32,
    pub(crate) iterations: usize,
}

/// What one clip pass found.
#[derive(Debug)]
enum ClipPass {
    /// No value was clipped, or the spread is below what the data can resolve: the final location.
    Converged(MedianMad),
    /// The survivors, now `kept` long at the front, the first `left` of them none above any after.
    Clipped { kept: usize, left: usize },
}

/// The values a clip runs on, as the keys that stand for them, each read by `decode`, beside the
/// scratch their deviations go in, as long as the keys.
#[derive(Debug)]
struct Clipping<'a, D> {
    keys: &'a mut [i32],
    deviations: &'a mut [f32],
    decode: D,
}

impl<D: Fn(i32) -> f32 + Copy> Clipping<'_, D> {
    /// Iteratively clip the values, `iterations` passes at most at `kappa` σ, then measure what
    /// survived: the first `count` keys.
    fn clip(mut self, clip: SigmaClip) -> MedianMad {
        let SigmaClip { kappa, iterations } = clip;
        let (mut len, mut boundary) = (self.keys.len(), 0);
        for _ in 0..iterations {
            if len < 3 {
                break;
            }
            match self.pass(len, boundary, kappa) {
                ClipPass::Converged(location) => return location,
                ClipPass::Clipped { kept, left } => (len, boundary) = (kept, left),
            }
        }
        // A pass keeps at least the values at the median, so `len` stays at least 1.
        self.location(len, boundary)
    }

    /// One pass over the first `len` values, at least 3, the keys before `boundary` none above any
    /// after: their median and MAD, then every value beyond `kappa` σ of the median dropped, the
    /// survivors' keys closed up at the front.
    ///
    /// The median leaves the keys partitioned at the middle, and the survivors keep their order, so
    /// the next pass's median is searched for on one side of the survivors of the middle's left.
    fn pass(&mut self, len: usize, boundary: usize, kappa: f32) -> ClipPass {
        debug_assert!(len >= 3 && self.deviations.len() == self.keys.len());
        let decode = self.decode;
        let median = statistics::median_of_partitioned_keys::<f32>(&mut self.keys[..len], boundary);
        self.fill_deviations(len, median);
        let mad = statistics::median_of_magnitudes(&mut self.deviations[..len]);
        let sigma = statistics::sigma_of_mad(mad, len);
        if sigma <= Spread::resolution(median) {
            return ClipPass::Converged(MedianMad {
                median,
                mad: 0.0,
                count: len,
            });
        }

        // The selection reordered the deviations, so each is taken again from its value. Every key
        // is written and the survivors' count alone moves, so no branch hangs on a value.
        let threshold = kappa * sigma;
        let keys = &mut self.keys[..len];
        let mut kept = 0;
        let mut close_up = |range: Range<usize>, kept: &mut usize| {
            for i in range {
                let key = keys[i];
                keys[*kept] = key;
                *kept += usize::from((decode(key) - median).abs() <= threshold);
            }
        };
        close_up(0..len / 2, &mut kept);
        let left = kept;
        close_up(len / 2..len, &mut kept);
        if kept == len {
            ClipPass::Converged(MedianMad {
                median,
                mad,
                count: len,
            })
        } else {
            ClipPass::Clipped { kept, left }
        }
    }

    /// The median and MAD of the first `len` values, under the total order.
    fn location(&mut self, len: usize, boundary: usize) -> MedianMad {
        debug_assert!(len > 0 && self.deviations.len() == self.keys.len());
        let median = statistics::median_of_partitioned_keys::<f32>(&mut self.keys[..len], boundary);
        self.fill_deviations(len, median);
        MedianMad {
            median,
            mad: statistics::median_of_magnitudes(&mut self.deviations[..len]),
            count: len,
        }
    }

    /// `|x − median|` for each of the first `len` values.
    fn fill_deviations(&mut self, len: usize, median: f32) {
        for (deviation, &key) in self.deviations[..len].iter_mut().zip(&self.keys[..len]) {
            *deviation = ((self.decode)(key) - median).abs();
        }
    }
}

/// Iteratively clip `values`, then measure what survived.
///
/// `deviations` must already be `values.len()` long — [`ClippedStats::sigma_clipped`] sizes it
/// through [`DeviationScratch`], and everything after the sizing lives here.
fn sigma_clipped_core(values: &mut [f32], deviations: &mut [f32], clip: SigmaClip) -> ClippedStats {
    // The values are clipped as their total-order keys, mapped at most once each way: each pass
    // selects twice, and the keys select with nothing mapped. Keys left unmapped are the values'
    // own bits, which read back as the values with no flip.
    let mut order = OrderKeys::of(values);
    let location = if order.mapped() {
        Clipping {
            keys: order.keys(),
            deviations,
            decode: f32::from_key,
        }
        .clip(clip)
    } else {
        Clipping {
            keys: order.keys(),
            deviations,
            decode: |key: i32| f32::from_bits(key.cast_unsigned()),
        }
        .clip(clip)
    };
    let values = order.into_values();
    ClippedStats::from_median_mad(location, sum::mean_f32(&values[..location.count]))
}
