//! [`ClippedStats`]: the sigma-clipped median, σ and mean of a sample.

use std::iter;

use arrayvec::ArrayVec;

use crate::math::statistics::median_mad::MedianMad;
use crate::math::statistics::spread::Spread;
use crate::math::statistics::{
    abs_deviation_inplace, mad_to_sigma, median_mut, median_of_magnitudes,
};
use crate::math::sum::mean_f32;

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
    /// MAD-based sigma of the survivors.
    pub(crate) sigma: f32,
    /// Mean of the survivors. With the median it exposes the residual skew of the clipped
    /// distribution — what SExtractor's Pearson mode estimator corrects for.
    pub(crate) mean: f32,
}

impl ClippedStats {
    const ZERO: ClippedStats = ClippedStats {
        median: 0.0,
        sigma: 0.0,
        mean: 0.0,
    };

    /// Widen a clip pass's estimate with the mean of the same survivors.
    const fn from_median_mad(location: MedianMad, mean: f32) -> Self {
        Self {
            median: location.median,
            sigma: location.sigma(),
            mean,
        }
    }

    /// Sigma-clipped median, MAD-sigma and mean of `values`, rejecting outliers beyond
    /// `kappa × sigma` from the median over `iterations` passes.
    ///
    /// `values` must be NaN-free, as a NaN never passes the clip's band and so has no place among
    /// the survivors whose mean this reports, and is reordered in place.
    ///
    /// `deviations` is borrowed scratch, overwritten in full; only its length matters. Sizing it
    /// is [`DeviationScratch`]'s job, so a heap caller can reuse one `Vec` across calls and a
    /// stamp-sized caller can keep its scratch on the stack, without this having to know which.
    pub(crate) fn sigma_clipped(
        values: &mut [f32],
        deviations: &mut impl DeviationScratch,
        kappa: f32,
        iterations: usize,
    ) -> Self {
        if values.is_empty() {
            return ClippedStats::ZERO;
        }
        debug_assert!(
            !values.iter().any(|value| value.is_nan()),
            "sigma_clipped requires NaN-free values"
        );

        sigma_clipped_core(values, deviations.sized_to(values.len()), kappa, iterations)
    }
}

/// What one clip pass found.
#[derive(Debug)]
enum ClipPass {
    /// No value was clipped, or the spread is below what the data can resolve: the final location.
    Converged(MedianMad),
    /// The survivors, now `kept` long at the front.
    Clipped { kept: usize },
}

/// One pass over `values`, at least 3 long: their median and MAD, then every value beyond `kappa`
/// σ of the median dropped, the survivors closed up at the front.
fn clip_pass(values: &mut [f32], deviations: &mut [f32], kappa: f32) -> ClipPass {
    debug_assert!(values.len() >= 3 && deviations.len() == values.len());
    let median = median_mut(values);
    deviations.copy_from_slice(values);
    abs_deviation_inplace(deviations, median);
    let mad = median_of_magnitudes(deviations);
    let sigma = mad_to_sigma(mad);
    if sigma <= Spread::resolution(median) {
        return ClipPass::Converged(MedianMad { median, mad: 0.0 });
    }

    // The selection reordered the deviations, so each is taken again from its value.
    let threshold = kappa * sigma;
    let mut kept = 0;
    for i in 0..values.len() {
        if (values[i] - median).abs() <= threshold {
            values[kept] = values[i];
            kept += 1;
        }
    }
    if kept == values.len() {
        ClipPass::Converged(MedianMad { median, mad })
    } else {
        ClipPass::Clipped { kept }
    }
}

/// The median and MAD of the survivors, under the total order, through `deviations`.
fn survivors_location(values: &mut [f32], deviations: &mut [f32]) -> MedianMad {
    debug_assert!(!values.is_empty() && deviations.len() == values.len());
    let median = median_mut(values);
    deviations.copy_from_slice(values);
    abs_deviation_inplace(deviations, median);
    MedianMad {
        median,
        mad: median_of_magnitudes(deviations),
    }
}

/// Iteratively clip `values`, then measure what survived.
///
/// `deviations` must already be `values.len()` long — [`ClippedStats::sigma_clipped`] sizes it
/// through [`DeviationScratch`], and everything after the sizing lives here.
fn sigma_clipped_core(
    values: &mut [f32],
    deviations: &mut [f32],
    kappa: f32,
    iterations: usize,
) -> ClippedStats {
    let mut len = values.len();
    let mut converged = None;
    for _ in 0..iterations {
        if len < 3 {
            break;
        }
        match clip_pass(&mut values[..len], &mut deviations[..len], kappa) {
            ClipPass::Converged(location) => {
                converged = Some(location);
                break;
            }
            ClipPass::Clipped { kept } => len = kept,
        }
    }

    // A pass keeps at least the values at the median, so `len` stays at least 1.
    let location =
        converged.unwrap_or_else(|| survivors_location(&mut values[..len], &mut deviations[..len]));
    ClippedStats::from_median_mad(location, mean_f32(&values[..len]))
}
